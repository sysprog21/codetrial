use super::*;

// RemoteParticipant has no public constructor. Let the pinned SDK construct it
// from a loopback JoinResponse and a local data-only WebRTC peer. No SFU,
// credentials, remote ICE servers or vendored test constructor is needed.
async fn candidate_participant() -> RemoteParticipant {
    let (room, _events, _arrive, server) = candidate_room("candidate", "{}", true).await;
    let candidate = room.remote_participants().into_values().next().unwrap();
    assert_eq!(candidate.identity().0, "candidate");
    startup_fixture_io("participant room close", room.close())
        .await
        .unwrap();
    startup_fixture_io("participant signal close", server)
        .await
        .unwrap();
    candidate
}

pub(crate) async fn candidate_room(
    identity: &str,
    metadata: &str,
    existing: bool,
) -> (
    Room,
    mpsc::UnboundedReceiver<RoomEvent>,
    tokio::sync::oneshot::Sender<()>,
    StartupPeer<()>,
) {
    // Native peer startup shares libwebrtc's network threads. Serialize only
    // this fixture's handshakes, not the interview or discovery under test.
    static CONNECTING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let _connecting = CONNECTING.lock().await;
    fn field(tag: u8, bytes: &[u8]) -> Vec<u8> {
        let mut result = vec![(tag << 3) | 2];
        let mut length = bytes.len();
        while length >= 128 {
            result.push((length as u8 & 127) | 128);
            length >>= 7;
        }
        result.push(length as u8);
        result.extend(bytes);
        result
    }
    fn varint(bytes: &mut &[u8]) -> usize {
        let mut result = 0;
        for shift in (0..64).step_by(7) {
            let byte = bytes[0];
            *bytes = &bytes[1..];
            result |= usize::from(byte & 127) << shift;
            if byte < 128 {
                return result;
            }
        }
        panic!("invalid fixture varint");
    }
    fn value(mut bytes: &[u8], tag: usize) -> Option<&[u8]> {
        while !bytes.is_empty() {
            let key = varint(&mut bytes);
            if key & 7 == 0 {
                varint(&mut bytes);
                continue;
            }
            assert_eq!(key & 7, 2);
            let length = varint(&mut bytes);
            let (value, rest) = bytes.split_at(length);
            if key >> 3 == tag {
                return Some(value);
            }
            bytes = rest;
        }
        None
    }
    let room = [field(1, b"RM_test"), field(2, b"presence-test")].concat();
    let local = [field(1, b"PA_agent"), field(2, b"agent")].concat();
    let candidate = [
        field(1, b"PA_candidate"),
        field(2, identity.as_bytes()),
        field(5, metadata.as_bytes()),
        vec![3 << 3, 2],
    ]
    .concat();
    let join = [
        field(1, &room),
        field(2, &local),
        if existing {
            field(3, &candidate)
        } else {
            Vec::new()
        },
        vec![10 << 3, 30, 11 << 3, 10], // ping timeout and interval
    ]
    .concat();
    let response = field(1, &join);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    let (arrive, mut arrival) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        use ::livekit::webrtc::prelude::*;
        let factory = PeerConnectionFactory::default();
        let peer = factory.create_peer_connection(Default::default()).unwrap();
        let (ice, mut candidates) = mpsc::channel(32);
        peer.on_ice_candidate(Some(Box::new(move |candidate| {
            let _ = ice.try_send(serde_json::json!({
                "candidate": candidate.candidate(),
                "sdpMid": candidate.sdp_mid(),
                "sdpMLineIndex": candidate.sdp_mline_index(),
            }));
        })));
        let (socket, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
        socket.send(Message::Binary(response.into())).await.unwrap();
        let mut arrived = existing;
        let mut remote_ready = false;
        let mut pending_ice = Vec::new();
        loop {
            tokio::select! {
                Ok(()) = &mut arrival, if !arrived => {
                    let update = field(5, &field(1, &candidate));
                    socket.send(Message::Binary(update.into())).await.unwrap();
                    arrived = true;
                }
                message = socket.next() => {
                    let Some(Ok(message)) = message else { break };
                    if message.is_close() { break; }
                    let Message::Binary(bytes) = message else { continue };
                    if let Some(offer) = value(&bytes, 1) {
                        let sdp = std::str::from_utf8(value(offer, 2).unwrap()).unwrap();
                        peer.set_remote_description(SessionDescription::parse(sdp, SdpType::Offer).unwrap()).await.unwrap();
                        remote_ready = true;
                        for candidate in std::mem::take(&mut pending_ice) {
                            peer.add_ice_candidate(candidate).await.unwrap();
                        }
                        let answer = peer.create_answer(Default::default()).await.unwrap();
                        let sdp = answer.to_string();
                        peer.set_local_description(answer).await.unwrap();
                        let answer = [field(1, b"answer"), field(2, sdp.as_bytes())].concat();
                        socket.send(Message::Binary(field(2, &answer).into())).await.unwrap();
                    } else if let Some(trickle) = value(&bytes, 3) {
                        let ice: serde_json::Value = serde_json::from_slice(value(trickle, 1).unwrap()).unwrap();
                        let candidate = IceCandidate::parse(
                            ice["sdpMid"].as_str().unwrap(),
                            ice["sdpMLineIndex"].as_i64().unwrap() as i32,
                            ice["candidate"].as_str().unwrap(),
                        ).unwrap();
                        // The SDK's ICE callback can beat its SDP offer.
                        if remote_ready {
                            peer.add_ice_candidate(candidate).await.unwrap();
                        } else {
                            assert!(pending_ice.len() < 32, "bounded fixture ICE prefix");
                            pending_ice.push(candidate);
                        }
                    }
                }
                Some(candidate) = candidates.recv() => {
                    let trickle = field(1, serde_json::to_string(&candidate).unwrap().as_bytes());
                    socket.send(Message::Binary(field(4, &trickle).into())).await.unwrap();
                }
            }
        }
        peer.close();
    });
    let server = StartupPeer(server);
    let mut options = RoomOptions::default();
    options.single_peer_connection = false;
    options.join_retries = 0;

    // Native ICE setup can outlast a short deadline when other test runtimes
    // share libwebrtc's threads. This bounds setup, not candidate discovery or
    // the virtual publication deadlines the tests assert after connecting.
    let (room, events) = tokio::time::timeout(
        Duration::from_secs(15),
        Room::connect(&url, "test-token", options),
    )
    .await
    .unwrap()
    .unwrap();
    (room, events, arrive, server)
}

#[tokio::test]
async fn startup_candidate_discovery_preserves_identity_metadata_and_deferred_events() {
    use futures_util::FutureExt;
    let identity = "candidate-metadata-owner";
    let metadata = serde_json::json!({
        "candidateIdentity": identity, "problemId": "merge-intervals", "durationMin": 30,
        "candidateLed": true, "interviewLoop": "coding_only",
        "interviewProfile": { "role": "Platform engineer", "seniority": "staff", "targetCompany": "Example Co" },
    }).to_string();
    for existing in [true, false] {
        let (room, mut events, arrive, server) =
            candidate_room(identity, &metadata, existing).await;
        assert_eq!(room.remote_participants().len(), usize::from(existing));

        // Put a non-participant event before the real SDK arrival. Discovery
        // must retain its observation time for the startup replay consumer.
        let (tx, mut discovery_events) = mpsc::unbounded_channel();
        tx.send(RoomEvent::Reconnecting).unwrap();
        let mut deferred = Vec::new();
        let before = Instant::now();
        let found = {
            let discovery = candidate_metadata(&room, &mut discovery_events, &mut deferred);
            tokio::pin!(discovery);
            if !existing {
                assert!(discovery.as_mut().now_or_never().is_none());
                arrive.send(()).unwrap();
                startup_fixture_io("SDK candidate arrival", async {
                    loop {
                        let event = events.recv().await.expect("room stays open");
                        let candidate = matches!(&event, RoomEvent::ParticipantConnected(p) if p.identity().0 == identity);
                        tx.send(event).unwrap();
                        if candidate { break; }
                    }
                }).await;
            }
            startup_fixture_io("candidate discovery", discovery)
                .await
                .unwrap()
                .unwrap()
        };
        assert_eq!(found, (identity.to_string(), metadata.clone()));
        let config = startup_config("60");
        let boot = candidate_bootstrap(&config, "presence-test", Some(&found.1));
        assert_eq!(boot.problem.id, "merge-intervals");
        assert_eq!(boot.duration_min, 30);
        assert!(boot.candidate_led_enabled);
        if existing {
            assert!(deferred.is_empty());
            assert!(matches!(
                discovery_events.try_recv(),
                Ok(RoomEvent::Reconnecting)
            ));
        } else {
            assert!(matches!(deferred[0].1, RoomEvent::Reconnecting));
            assert!(before <= deferred[0].0 && deferred[0].0 <= Instant::now());
            let observed_at = deferred[0].0;
            let events = tokio::sync::Mutex::new(RoomEventReader {
                events: discovery_events,
                deferred: deferred.into(),
            });
            let (at, event) = next_timed_room_event(&events).await.unwrap();
            assert_eq!(at, observed_at);
            assert!(matches!(event, RoomEvent::Reconnecting));
        }
        startup_fixture_io("discovery room close", room.close())
            .await
            .unwrap();
        startup_fixture_io("discovery peer close", server)
            .await
            .unwrap();
    }
}

// Keep paused time fixed during real loopback IO. Only explicit advance calls
// move the publication/presence clock, never Tokio's idle auto-advance.
async fn io_at_fixed_time<T>(future: impl Future<Output = T>) -> T {
    startup_fixture_io("publication loopback IO", future).await
}

#[tokio::test]
async fn startup_publication_hands_deferred_end_and_skip_to_report_handler() {
    use futures_util::FutureExt;

    let candidate = candidate_participant().await;
    let config = startup_config("60");
    // A broken handoff must fail locally rather than contact the report API.
    let keys = Arc::new(GeminiKeys::single(""));
    let boot = crate::runtime::bootstrap(&config, "handoff-test", Some("two-sum"), 45);
    let (mut gemini, requested, answer, server) = reconnect_socket(&boot).await;
    answer.send(true).unwrap();
    let mut turn = hold_turn(initial_runtime_state(&boot, Instant::now()));
    let ready_at = startup_fixture_io(
        "handoff opening output",
        probe_live_output(
            &mut gemini,
            &mut turn.state,
            "opening",
            startup_output_limit(&config),
        ),
    )
    .await
    .unwrap();
    requested.await.unwrap();
    turn.state.started_at = ready_at;
    let (room, sent) = RoomLink::recording(boot.room_name);
    let (tx, receiver) = mpsc::unbounded_channel();
    let events = tokio::sync::Mutex::new(RoomEventReader::from(receiver));
    let packet = |message: serde_json::Value, participant| RoomEvent::DataReceived {
        topic: Some(TOPIC_CONTROL.into()),
        payload: Arc::new(serde_json::to_vec(&message).unwrap()),
        kind: ::livekit::DataPacketKind::Reliable,
        participant,
    };
    let (release, released) = tokio::sync::oneshot::channel();
    let mut deferred = Vec::new();
    let mut usage = Default::default();
    let mut closed = false;
    {
        let publishing = crate::livekit::publish_startup_readiness(
            &mut gemini,
            &mut usage,
            &events,
            &mut deferred,
            &mut closed,
            async {
                publish_interviewer_state(&room, &startup_ready_notice(ready_at, None)).await?;
                released.await.unwrap();
                Ok(())
            },
        );
        tokio::pin!(publishing);
        assert!(publishing.as_mut().now_or_never().is_none());
        assert!(
            matches!(&sent.take()[0], room_link::Sent::Data { message, .. } if message["startup"] == "ready")
        );
        tx.send(packet(
            serde_json::json!({"type":"end_interview", "reason":"candidate_ended"}),
            Some(candidate.clone()),
        ))
        .unwrap();
        tx.send(packet(serde_json::json!({"type":"skip_report"}), None))
            .unwrap();
        tx.send(packet(
            serde_json::json!({"type":"skip_report"}),
            Some(candidate),
        ))
        .unwrap();
        assert!(publishing.as_mut().now_or_never().is_none());
        release.send(()).unwrap();
        startup_fixture_io("held ready completion", publishing)
            .await
            .unwrap();
    }
    assert!(!closed);
    assert_eq!(deferred.len(), 3);
    let end_at = deferred[0].0;
    events.lock().await.deferred = deferred.into();
    // New SDK events must remain behind all the publication-time packets.
    tx.send(RoomEvent::Reconnecting).unwrap();
    let end = next_timed_room_event(&events).await.unwrap();
    assert_eq!(end.0, end_at);
    let interview = InterviewContext {
        config: &config,
        keys: &keys,
        boot: &boot,
        started_at: ready_at,
        candidate_identity: "candidate",
        events: &events,
        slot: None,
    };
    let ids = RoomIdentities {
        room_name: boot.room_name,
        agent: "agent",
        candidate: "candidate",
        now_seconds: 0,
    };
    let (mut audio, _frames) = test_output_audio();
    let mut board = Board::new();
    let mut media = CandidateMedia::new();
    let mut presence = CandidatePresence::default();
    let mut context = turn.context(&mut audio, &mut gemini, &mut board, &mut media);
    assert!(
        startup_fixture_io(
            "deferred End to report Skip",
            handle_room_event(&room, &mut context, &mut presence, interview, &ids, end),
        )
        .await
        .unwrap()
        .is_break()
    );
    assert!(context.state.ended, "End was reduced before Skip");
    let output = sent.take();
    let ack = output.iter().position(|event| matches!(event, room_link::Sent::Data { message, .. } if message["type"] == "skip_report_ack")).expect("queued Skip was acknowledged");
    let left = output
        .iter()
        .position(|event| matches!(event, room_link::Sent::Left))
        .unwrap();
    assert!(ack < left, "ACK must precede leaving");
    assert!(!output.iter().any(|event| matches!(event, room_link::Sent::Data { topic, .. } if topic.as_deref() == Some(crate::runtime::TOPIC_REPORT))));
    assert!(events.lock().await.deferred.is_empty());
    assert!(matches!(
        events.lock().await.try_recv(),
        Ok(RoomEvent::Reconnecting)
    ));
    assert_eq!(
        startup_fixture_io("handoff Gemini close", server)
            .await
            .unwrap(),
        1
    );
}

#[tokio::test]
async fn startup_publication_watch_first_preserves_received_returns() {
    publication_presence(true).await;
}

#[tokio::test]
async fn startup_publication_event_first_preserves_received_returns() {
    publication_presence(false).await;
}

async fn publication_presence(watch_first: bool) {
    use futures_util::FutureExt;
    let candidate = candidate_participant().await;
    for returned_after in [
        Some(Duration::from_secs(89)),
        Some(CANDIDATE_ABSENCE_LIMIT - Duration::from_nanos(1)),
        Some(CANDIDATE_ABSENCE_LIMIT),
        Some(CANDIDATE_ABSENCE_LIMIT + Duration::from_nanos(1)),
        None,
    ] {
        tokio::time::pause();
        let config = startup_config("120");
        assert!(startup_output_limit(&config) >= Duration::from_secs(82));
        let keys = Arc::new(GeminiKeys::from_config(&config));
        let boot = crate::runtime::bootstrap(&config, "presence-test", Some("two-sum"), 45);
        let (tx, receiver) = mpsc::unbounded_channel();
        let events = tokio::sync::Mutex::new(RoomEventReader::from(receiver));
        let mut deferred = std::collections::VecDeque::new();
        tx.send(RoomEvent::ParticipantDisconnected(candidate.clone()))
            .unwrap();
        let departure = next_timed_room_event(&events).await.unwrap();
        let left_at = departure.0;
        deferred.push_back(departure);
        tokio::time::advance(Duration::from_secs(81)).await;

        let (mut gemini, requested, answer, server) =
            io_at_fixed_time(reconnect_socket(&boot)).await;
        let mut turn = hold_turn(initial_runtime_state(&boot, Instant::now()));
        answer.send(true).unwrap();
        let ready_at = io_at_fixed_time(probe_live_output(
            &mut gemini,
            &mut turn.state,
            "opening",
            startup_output_limit(&config),
        ))
        .await
        .unwrap();
        requested.await.unwrap();
        turn.state.started_at = ready_at;

        // The virtual clock controls presence/publication only. Keep the
        // unrelated idle watchdog at the current virtual interview age.
        turn.activity = runtime_activity(&config, tokio::time::Instant::now().into_std(), 7);
        let (room, sent) = RoomLink::recording(boot.room_name);
        let ready = startup_ready_notice(ready_at, Some(7));
        let (release, released) = tokio::sync::oneshot::channel();
        let mut usage = Default::default();
        let mut deferred = Vec::from(deferred);
        let mut room_closed = false;
        {
            let publishing = crate::livekit::publish_startup_readiness(
                &mut gemini,
                &mut usage,
                &events,
                &mut deferred,
                &mut room_closed,
                async {
                    released.await.unwrap();
                    publish_interviewer_state(&room, &ready).await
                },
            );
            tokio::pin!(publishing);
            assert!(publishing.as_mut().now_or_never().is_none());
            tokio::time::advance(Duration::from_secs(1)).await;
            tx.send(RoomEvent::ParticipantDisconnected(candidate.clone()))
                .unwrap();
            tx.send(RoomEvent::Reconnecting).unwrap();
            assert!(publishing.as_mut().now_or_never().is_none());
            if let Some(after) = returned_after {
                tokio::time::advance(after - Duration::from_secs(82)).await;
                tx.send(RoomEvent::ParticipantConnected(candidate.clone()))
                    .unwrap();
                assert_eq!(tokio::time::Instant::now().into_std(), left_at + after);
                assert!(publishing.as_mut().now_or_never().is_none());
            }
            let finish_at = tokio::time::Instant::from_std(left_at + Duration::from_secs(94));
            tokio::time::advance(finish_at - tokio::time::Instant::now()).await;
            release.send(()).unwrap();
            io_at_fixed_time(publishing).await.unwrap();
        }
        let deferred = std::collections::VecDeque::from(deferred);
        assert!(!room_closed);
        assert_eq!(deferred.len(), if returned_after.is_some() { 4 } else { 3 });
        assert_eq!(deferred[0].0, left_at);
        assert_eq!(deferred[1].0, left_at + Duration::from_secs(82));
        assert_eq!(deferred[2].0, left_at + Duration::from_secs(82));
        if let Some(after) = returned_after {
            assert_eq!(
                deferred[3].0,
                left_at + after,
                "capture precedes publication completion"
            );
        }
        assert_eq!(sent.take().len(), 1, "one initial ready notice");
        let timely = returned_after.is_some_and(|after| after < CANDIDATE_ABSENCE_LIMIT);
        events.lock().await.deferred = deferred;
        let mut loops = RoomLoop {
            restarts: 0,
            deferred_restart: Default::default(),
            resume_recovery: Default::default(),
            interim_review: Default::default(),
            presence: Default::default(),
        };
        let interview = InterviewContext {
            config: &config,
            keys: &keys,
            boot: &boot,
            started_at: ready_at,
            candidate_identity: "candidate",
            events: &events,
            slot: None,
        };
        let ids = RoomIdentities {
            room_name: boot.room_name,
            agent: "agent",
            candidate: "candidate",
            now_seconds: 0,
        };
        let (mut audio, _frames) = test_output_audio();
        let mut board = Board::new();
        let mut media = CandidateMedia::new();
        let mut context = turn.context(&mut audio, &mut gemini, &mut board, &mut media);
        if watch_first {
            let step = io_at_fixed_time(on_watch_tick(
                &room,
                &mut context,
                &mut loops,
                interview,
                tokio::time::Instant::now().into_std(),
            ))
            .await
            .unwrap();
            assert_eq!(
                step.is_continue(),
                timely,
                "watch-first return={returned_after:?}"
            );
            assert!(
                loops.presence.left_at.is_none(),
                "projection has no side effects"
            );
        }
        if !watch_first || timely {
            let mut timestamps = Vec::new();
            for index in 0..if returned_after.is_some() { 4 } else { 3 } {
                let event = next_timed_room_event(&events).await.unwrap();
                timestamps.push(event.0);
                if index == 2 {
                    assert!(matches!(event.1, RoomEvent::Reconnecting));
                }
                assert!(
                    io_at_fixed_time(handle_room_event(
                        &room,
                        &mut context,
                        &mut loops.presence,
                        interview,
                        &ids,
                        event
                    ))
                    .await
                    .unwrap()
                    .is_continue()
                );
            }
            assert_eq!(
                loops.presence.left_at.is_none(),
                timely,
                "event-first return={returned_after:?}"
            );
            assert_eq!(timestamps[0], left_at);
            assert_eq!(timestamps[1], left_at + Duration::from_secs(82));
            if let Some(after) = returned_after {
                assert_eq!(timestamps[3], left_at + after);
            }
            let step = io_at_fixed_time(on_watch_tick(
                &room,
                &mut context,
                &mut loops,
                interview,
                tokio::time::Instant::now().into_std(),
            ))
            .await
            .unwrap();
            assert_eq!(step.is_continue(), timely);
        }
        assert_eq!(
            sent.take()
                .iter()
                .any(|event| matches!(event, room_link::Sent::Left)),
            !timely
        );
        io_at_fixed_time(gemini.shutdown()).await.unwrap();
        assert_eq!(io_at_fixed_time(server).await.unwrap(), 1);
        tokio::time::resume();
    }
}

#[tokio::test]
async fn startup_publication_collection_keeps_failure_bounds_and_ownership() {
    use futures_util::FutureExt;
    use std::sync::atomic::{AtomicBool, Ordering};
    struct Dropped(Arc<AtomicBool>);
    impl Drop for Dropped {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
    for outcome in [
        "error",
        "timeout",
        "disconnect",
        "closed",
        "capacity",
        "overflow",
        "cancel",
    ] {
        let config = startup_config("120");
        let boot = crate::runtime::bootstrap(&config, "publication-failure", Some("two-sum"), 45);
        let (mut gemini, requested, answer, server) = reconnect_socket(&boot).await;
        answer.send(true).unwrap();
        startup_fixture_io(
            "publication fixture opening output",
            probe_live_output(
                &mut gemini,
                &mut startup_state_for(&config),
                "opening",
                startup_output_limit(&config),
            ),
        )
        .await
        .unwrap();
        requested.await.unwrap();
        let (_, reader) = gemini.startup_reader_observer();
        let (tx, receiver) = mpsc::unbounded_channel();
        let events = tokio::sync::Mutex::new(RoomEventReader::from(receiver));
        let mut deferred = Vec::new();
        let mut room_closed = false;
        let mut usage = Default::default();
        let dropped = Arc::new(AtomicBool::new(false));
        let guard = Dropped(dropped.clone());
        let (release, released) = tokio::sync::oneshot::channel();
        tokio::time::pause();
        let began = tokio::time::Instant::now();
        if matches!(outcome, "capacity" | "overflow") {
            deferred.extend(
                (0..STARTUP_READY_EVENT_LIMIT - 1)
                    .map(|_| (began.into_std(), RoomEvent::Reconnecting)),
            );
        }
        let result = {
            let publishing = crate::livekit::publish_startup_readiness(
                &mut gemini,
                &mut usage,
                &events,
                &mut deferred,
                &mut room_closed,
                async move {
                    let _guard = guard;
                    released.await.unwrap();
                    if outcome == "error" {
                        Err(std::io::Error::other("notice rejected").into())
                    } else {
                        Ok(())
                    }
                },
            );
            tokio::pin!(publishing);
            assert!(publishing.as_mut().now_or_never().is_none());
            tx.send(RoomEvent::Reconnecting).unwrap();
            assert!(publishing.as_mut().now_or_never().is_none());
            match outcome {
                "cancel" => None,
                "timeout" => {
                    tokio::time::advance(STARTUP_READY_PUBLICATION_LIMIT - Duration::from_secs(1))
                        .await;
                    tx.send(RoomEvent::Reconnected).unwrap();
                    assert!(publishing.as_mut().now_or_never().is_none());
                    tokio::time::advance(Duration::from_millis(1001)).await;
                    Some(io_at_fixed_time(publishing).await)
                }
                "disconnect" => {
                    tx.send(RoomEvent::Disconnected {
                        reason: DisconnectReason::ClientInitiated,
                    })
                    .unwrap();
                    Some(io_at_fixed_time(publishing).await)
                }
                "closed" => {
                    drop(tx);
                    Some(io_at_fixed_time(publishing).await)
                }
                "overflow" => {
                    tx.send(RoomEvent::Reconnected).unwrap();
                    Some(io_at_fixed_time(publishing).await)
                }
                _ => {
                    release.send(()).unwrap();
                    Some(io_at_fixed_time(publishing).await)
                }
            }
        };
        assert!(
            dropped.load(Ordering::SeqCst),
            "publication future is owned and dropped"
        );
        assert!(
            events.try_lock().is_ok(),
            "no receiver borrow survives handoff"
        );
        assert_eq!(room_closed, matches!(outcome, "closed" | "disconnect"));
        assert!(matches!(deferred[0].1, RoomEvent::Reconnecting));
        assert_eq!(deferred[0].0, began.into_std());
        if matches!(outcome, "capacity" | "overflow") {
            assert_eq!(deferred.len(), STARTUP_READY_EVENT_LIMIT);
        } else {
            assert_eq!(deferred.len(), if outcome == "timeout" { 2 } else { 1 });
        }
        if let Some(result) = result {
            if outcome == "capacity" {
                result.unwrap();
                assert!(!reader.is_finished());
            } else {
                let error = result.unwrap_err();
                assert_eq!(startup_state(Some(error.as_ref()))["reason"], "delivery");
                assert!(reader.is_finished(), "failure joins the Gemini reader");
                assert!(gemini.drain_usage().is_empty());
                if outcome == "timeout" {
                    assert!(began.elapsed() >= STARTUP_READY_PUBLICATION_LIMIT);
                    assert!(
                        began.elapsed()
                            <= STARTUP_READY_PUBLICATION_LIMIT
                                + crate::gemini::CLOSE_TIMEOUT
                                + Duration::from_millis(1)
                    );
                }
            }
            io_at_fixed_time(gemini.shutdown()).await.unwrap();
        } else {
            assert!(
                !reader.is_finished(),
                "cancelled borrow leaves socket with its owner"
            );
            drop(gemini);
            io_at_fixed_time(async {
                while !reader.is_finished() {
                    tokio::task::yield_now().await;
                }
            })
            .await;
        }
        assert_eq!(io_at_fixed_time(server).await.unwrap(), 1);
        tokio::time::resume();
    }
}
