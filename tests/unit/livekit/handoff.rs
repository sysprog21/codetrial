use super::*;
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;

const OWED: &str = "[SYSTEM EVENT] Review the latest editor.";

struct TickFixture {
    config: AgentConfig,
    keys: Arc<GeminiKeys>,
    turn: TurnState,
    output: OutputAudio,
    _frames: mpsc::Receiver<QueuedOutputFrame>,
    gemini: GeminiLiveSession,
    server: tokio::task::JoinHandle<Vec<serde_json::Value>>,
    media: CandidateMedia,
    board: board::Board,
    room: room_link::RoomLink,
    loops: RoomLoop,
    events: tokio::sync::Mutex<mpsc::UnboundedReceiver<RoomEvent>>,
    _events: mpsc::UnboundedSender<RoomEvent>,
    endpoint: String,
}

impl TickFixture {
    async fn new(silence_ms: u32, first_output: u32) -> Self {
        let silence = silence_ms.to_string();
        let first = first_output.to_string();
        let config = load_from_pairs([
            ("LIVEKIT_URL", "wss://example.livekit.cloud"),
            ("LIVEKIT_API_KEY", "devkey"),
            ("LIVEKIT_API_SECRET", "devsecret"),
            ("GOOGLE_API_KEY", "sequence-test-key"),
            ("GEMINI_SILENCE_MS", &silence),
            ("CODETRIAL_GEMINI_FIRST_OUTPUT_S", &first),
        ])
        .unwrap();
        let keys = Arc::new(GeminiKeys::from_config(&config));
        let mut turn = hold_turn(RuntimeState::default());
        turn.activity = RuntimeActivity::new(Instant::now())
            .with_first_output_deadline(Duration::from_secs(u64::from(first_output)));
        let armed = Instant::now() - Duration::from_secs(5);
        turn.activity.owe_prompt(armed, Some(OWED.into()));
        turn.activity.arm_handoff_reply(armed);
        let (output, frames) = test_output_audio();

        // Zero waits for close, but records an unexpected prompt instead of
        // treating silence as a passing scripted send.
        let (gemini, server) = fake_recording_socket(None, 0).await;
        let (event_sender, events) = mpsc::unbounded_channel();
        let (room, _) = room_link::RoomLink::recording("handoff-tick");
        Self {
            config,
            keys,
            turn,
            output,
            _frames: frames,
            gemini,
            server,
            media: CandidateMedia::new(),
            board: board::Board::new(),
            room,
            loops: RoomLoop {
                pending_round_notice: None,
                recovery_terminal: None,
                recovery_inputs: None,
                recovery_phase: None,
                recovery_reader: None,
                recovery_writes: Default::default(),
                recovery_after: Default::default(),
                deadline_at: tokio::time::Instant::now() + Duration::from_secs(3600),
                restarts: 0,
                deferred_restart: DeferredRestart::default(),
                resume_recovery: ResumeRecovery::default(),
                interim_review: SideCall::default(),
                presence: CandidatePresence::default(),
            },
            events: tokio::sync::Mutex::new(events),
            _events: event_sender,
            endpoint: "ws://127.0.0.1:9/".into(),
        }
    }

    async fn tick(&mut self) {
        let boot = crate::runtime::bootstrap(&self.config, "handoff-tick", Some("two-sum"), 45);
        let interview = InterviewContext {
            config: &self.config,
            keys: &self.keys,
            boot: &boot,
            live_endpoint: Some(&self.endpoint),
            started_at: Instant::now(),
            candidate_identity: "candidate",
            events: &self.events,
            slot: None,
        };
        let flow = tokio::time::timeout(
            Duration::from_secs(3),
            on_watch_tick(
                &self.room,
                &mut self.turn.context(
                    &mut self.output,
                    &mut self.gemini,
                    &mut self.board,
                    &mut self.media,
                ),
                &mut self.loops,
                interview,
            ),
        )
        .await
        .expect("the handoff tick must not wait on a provider")
        .unwrap();
        assert!(flow.is_continue());
    }

    async fn replace_speech(&mut self, handoff: session::SpeechHandoff) {
        let boot = crate::runtime::bootstrap(&self.config, "handoff-tick", Some("two-sum"), 45);
        let interview = InterviewContext {
            config: &self.config,
            keys: &self.keys,
            boot: &boot,
            live_endpoint: Some(&self.endpoint),
            started_at: Instant::now(),
            candidate_identity: "candidate",
            events: &self.events,
            slot: None,
        };
        assert!(
            tokio::time::timeout(
                Duration::from_secs(10),
                replace_gemini_session(
                    &self.room,
                    &mut self.turn.context(
                        &mut self.output,
                        &mut self.gemini,
                        &mut self.board,
                        &mut self.media,
                    ),
                    interview,
                    &mut self.loops,
                    Trigger::GoAway,
                    Some(handoff),
                ),
            )
            .await
            .expect("the local speech handoff must complete")
            .unwrap()
            .is_continue()
        );
    }

    async fn finish(mut self) -> Vec<serde_json::Value> {
        self.gemini.shutdown().await.unwrap();
        tokio::time::timeout(Duration::from_secs(2), self.server)
            .await
            .expect("the local peer must finish after shutdown")
            .unwrap()
    }
}

#[tokio::test]
async fn a_tick_requests_the_quiet_handoff_once_before_the_same_tick_watchdog() {
    let mut fixture = TickFixture::new(1500, 5).await;
    assert_eq!(
        reply_watch(
            &fixture.turn.state,
            &fixture.turn.activity,
            Instant::now(),
            false
        ),
        ReplyWatch::Recover
    );
    let before = Instant::now();
    fixture.tick().await;
    assert!(fixture.turn.activity.prompted_at.unwrap() >= before);
    assert_eq!(fixture.turn.activity.prompt_text.as_deref(), Some(OWED));
    assert!(fixture.turn.activity.owes_prompt());
    assert_eq!(
        fixture.turn.state.evidence_ledger.metrics.turn_prompt_count,
        1
    );
    let requested = fixture.turn.activity.prompted_at.unwrap();
    assert_eq!(
        fixture
            .turn
            .activity
            .expired_reply_wait(requested + Duration::from_secs(9), false),
        None
    );
    assert_eq!(
        fixture
            .turn
            .activity
            .expired_reply_wait(requested + Duration::from_secs(10), false),
        Some(Duration::from_secs(10))
    );
    fixture.tick().await;
    assert_eq!(
        fixture.turn.state.evidence_ledger.metrics.turn_prompt_count,
        1
    );
    assert_eq!(fixture.loops.restarts, 0);
    let messages = fixture.finish().await;
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["clientContent"]["turnComplete"], true);
    assert!(sent_text(&messages[0]).contains("has finished speaking"));
}

#[tokio::test]
async fn a_failed_handoff_request_keeps_debt_and_the_original_deadline() {
    let mut fixture = TickFixture::new(1500, 10).await;
    fixture.gemini.shutdown().await.unwrap();
    let prompted = fixture.turn.activity.prompted_at;
    fixture.tick().await;
    fixture.tick().await;
    assert_eq!(fixture.turn.activity.prompted_at, prompted);
    assert_eq!(fixture.turn.activity.prompt_text.as_deref(), Some(OWED));
    assert!(fixture.turn.activity.owes_prompt());
    assert!(!fixture.turn.activity.handoff_reply_due(
        Instant::now() + Duration::from_secs(60),
        Duration::ZERO,
        false
    ));
    // Input accounting measures constructed input, even when transport fails.
    assert_eq!(
        fixture.turn.state.evidence_ledger.metrics.turn_prompt_count,
        1
    );
    assert!(fixture.finish().await.is_empty());
}

#[tokio::test]
async fn the_tick_keeps_handoff_requests_out_of_held_or_superseded_turns() {
    for case in [
        "playout",
        "provider",
        "microphone",
        "both",
        "thinking",
        "ending",
    ] {
        let mut fixture = TickFixture::new(1500, 10).await;
        let now = Instant::now();
        match case {
            "playout" => fixture.output.playout_deadline = now + Duration::from_secs(5),
            "provider" => fixture
                .turn
                .activity
                .note_candidate_voice_activity(true, now),
            "microphone" => fixture.turn.activity.note_candidate_voice(now),
            "both" => {
                fixture
                    .turn
                    .activity
                    .note_candidate_voice_activity(true, now);
                fixture.turn.activity.note_candidate_voice(now);
            }
            "thinking" => fixture.turn.state.thinking_hold = ThinkingHold::Held { since: now },
            "ending" => {
                fixture.turn.state.end_requested = true;
                fixture.turn.activity.socket.tool_response_outstanding = true;
            }
            _ => unreachable!(),
        }
        fixture.tick().await;
        assert_eq!(
            fixture.turn.state.evidence_ledger.metrics.turn_prompt_count, 0,
            "{case}"
        );
        fixture.turn.activity.clear_candidate_voice_activity();
        assert!(
            fixture.turn.activity.handoff_reply_due(
                now + Duration::from_secs(60),
                Duration::ZERO,
                false
            ),
            "{case}"
        );
        assert!(fixture.finish().await.is_empty(), "{case}");
    }
}

#[tokio::test]
async fn output_or_a_new_prompt_does_not_resurrect_a_handoff_on_the_next_tick() {
    for output in [true, false] {
        let mut fixture = TickFixture::new(1500, 10).await;
        let now = Instant::now();
        if output {
            fixture.turn.activity.note_output(now);
        } else {
            fixture
                .turn
                .activity
                .mark_prompted(now, Some("new prompt"), false);
        }
        assert!(!fixture.turn.activity.handoff_reply_due(
            now + Duration::from_secs(60),
            Duration::ZERO,
            false
        ));
        fixture.tick().await;
        assert_eq!(
            fixture.turn.state.evidence_ledger.metrics.turn_prompt_count,
            0
        );
        assert!(fixture.finish().await.is_empty());
    }
}

#[tokio::test]
async fn expiring_a_stale_voice_start_restarts_quiet_instead_of_prompting_immediately() {
    for timestamp in [None, Some(Instant::now() - Duration::from_secs(15))] {
        let mut fixture = TickFixture::new(1500, 10).await;
        fixture.turn.activity.candidate_voice_active = true;
        fixture.turn.activity.candidate_voice_at = timestamp;
        fixture.tick().await;
        assert!(!fixture.turn.activity.candidate_voice_active);
        assert!(fixture.turn.activity.candidate_voice_ended());
        assert_eq!(
            fixture.turn.state.evidence_ledger.metrics.turn_prompt_count,
            0
        );
        assert!(!fixture.turn.activity.handoff_reply_due(
            Instant::now(),
            Duration::from_millis(1500),
            false
        ));
        assert!(fixture.turn.activity.handoff_reply_due(
            Instant::now() + Duration::from_secs(5),
            Duration::from_millis(1500),
            false
        ));
        assert!(fixture.finish().await.is_empty());
    }
}

#[tokio::test]
async fn a_tick_recovers_when_long_quiet_or_a_failed_request_leaves_the_watchdog_due() {
    for (silence, first, age, failed_request) in [(30_000, 10, 11, false), (1500, 5, 5, true)] {
        let mut fixture = TickFixture::new(silence, first).await;
        let old = Instant::now() - Duration::from_secs(age);
        fixture.turn.activity.owe_prompt(old, Some(OWED.into()));
        fixture.turn.activity.arm_handoff_reply(old);
        if failed_request {
            fixture.gemini.shutdown().await.unwrap();
        }
        assert_eq!(
            reply_watch(
                &fixture.turn.state,
                &fixture.turn.activity,
                Instant::now(),
                false
            ),
            ReplyWatch::Recover
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        fixture.endpoint = format!("ws://{}/", listener.local_addr().unwrap());
        let peer = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
            let setup = socket.next().await.unwrap().unwrap();
            let setup: serde_json::Value = serde_json::from_str(setup.to_text().unwrap()).unwrap();
            assert_eq!(setup["setup"]["sessionResumption"], serde_json::json!({}));
            socket
                .send(Message::Text(r#"{"setupComplete":{}}"#.into()))
                .await
                .unwrap();
            let mut messages = Vec::new();
            loop {
                let message = tokio::time::timeout(Duration::from_secs(3), socket.next())
                    .await
                    .expect("the replacement must finish its briefing and close")
                    .unwrap()
                    .unwrap();
                if message.is_close() {
                    break;
                }
                messages.push(
                    serde_json::from_str::<serde_json::Value>(message.to_text().unwrap()).unwrap(),
                );
                assert!(messages.len() <= 2, "no repeated recovery request");
            }
            messages
        });
        let socket = fixture.turn.activity.live_socket;
        fixture.tick().await;
        assert_eq!(fixture.turn.activity.live_socket, socket + 1);
        assert_eq!(fixture.loops.restarts, 1);
        assert!(fixture.turn.activity.owes_prompt());
        assert_eq!(
            fixture.turn.state.evidence_ledger.metrics.turn_prompt_count,
            if failed_request { 2 } else { 1 }
        );
        assert!(!fixture.turn.activity.handoff_reply_due(
            Instant::now() + Duration::from_secs(60),
            Duration::ZERO,
            false
        ));
        assert!(
            fixture.finish().await.is_empty(),
            "the old socket received no handoff request"
        );
        let messages = tokio::time::timeout(Duration::from_secs(3), peer)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0]["clientContent"]["turnComplete"], false);
        assert_eq!(messages[1]["clientContent"]["turnComplete"], true);
        assert!(sent_text(&messages[1]).contains("Review the latest editor."));
    }
}

#[tokio::test]
async fn a_failed_listening_note_keeps_the_cold_brief_due_and_never_arms_the_tick() {
    let mut fixture = TickFixture::new(1500, 10).await;
    let listener = tokio::net::TcpSocket::new_v4().unwrap();
    listener.set_recv_buffer_size(1024).unwrap();
    listener.bind("127.0.0.1:0".parse().unwrap()).unwrap();
    let listener = listener.listen(1).unwrap();
    fixture.endpoint = format!("ws://{}/", listener.local_addr().unwrap());
    let peer = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
        let setup = socket.next().await.unwrap().unwrap();
        assert!(
            serde_json::from_str::<serde_json::Value>(setup.to_text().unwrap()).unwrap()["setup"]
                .is_object()
        );
        socket
            .send(Message::Text(r#"{"setupComplete":{}}"#.into()))
            .await
            .unwrap();

        // Reset only once the listening note is being sent. A small frame could
        // fit in the TCP buffer and falsely report a successful send.
        assert_eq!(socket.get_ref().peek(&mut [0]).await.unwrap(), 1);
        drop(socket);
    });
    fixture
        .replace_speech(session::SpeechHandoff {
            checkpoint_after: Instant::now(),
            transcript_fragments: vec!["x".repeat(32 * 1024 * 1024)],
            discarded: 0,
        })
        .await;
    assert!(fixture.turn.state.needs_cold_brief);
    assert!(fixture.turn.activity.owes_prompt());
    assert_eq!(fixture.turn.activity.prompt_text.as_deref(), Some(OWED));
    assert!(!fixture.turn.activity.handoff_reply_due(
        Instant::now() + Duration::from_secs(60),
        Duration::ZERO,
        false
    ));
    assert!(fixture.finish().await.is_empty());
    tokio::time::timeout(Duration::from_secs(3), peer)
        .await
        .unwrap()
        .unwrap();
}

#[test]
fn configured_handoff_quiet_and_recovery_deadlines_have_exact_boundaries() {
    let start = Instant::now();
    let state = RuntimeState::default();
    let mut cases = 0;
    for silence_ms in [0, 1500, 7000, 30_000] {
        for quiet_offset_ms in [0, 1500, 2500, 7000, 7500, 30_000] {
            for requested_first_s in [5, 10, 120] {
                for reply_s in [20, 45, 120] {
                    let first = Duration::from_secs(requested_first_s.min(reply_s));
                    let reply = Duration::from_secs(reply_s);
                    let silence = Duration::from_millis(silence_ms);
                    let quiet = start + Duration::from_millis(quiet_offset_ms);
                    let due = quiet + silence + Duration::from_millis(2500);
                    let mut activity = RuntimeActivity::new(start)
                        .with_first_output_deadline(first)
                        .with_reply_timeout(reply);
                    activity.owe_prompt(start, Some(OWED.into()));
                    activity.arm_handoff_reply(start);
                    activity.note_candidate_voice_activity(false, quiet);
                    assert!(!activity.handoff_reply_due(
                        due - Duration::from_millis(1),
                        silence,
                        false
                    ));
                    assert!(activity.handoff_reply_due(due, silence, false));
                    assert_eq!(
                        reply_watch(&state, &activity, due, false) == ReplyWatch::Recover,
                        due >= start + first
                    );
                    activity.renew_handoff_prompt(due);
                    let recovery = first.max(Duration::from_secs(10)).min(reply);
                    assert_eq!(
                        activity
                            .expired_reply_wait(due + recovery - Duration::from_millis(1), false),
                        None
                    );
                    assert_eq!(
                        activity.expired_reply_wait(due + recovery, false),
                        Some(recovery)
                    );
                    cases += 1;
                }
            }
        }
    }
    assert_eq!(cases, 216);
}
