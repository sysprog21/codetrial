use super::*;
use futures_util::{SinkExt, StreamExt};
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio_tungstenite::tungstenite::Message;

use super::super::readiness::{
    CandidatePresenceChange, RoomEventReader, STARTUP_READY_EVENT_LIMIT,
    STARTUP_READY_PUBLICATION_LIMIT, StartupContext, StartupDeliveryError, StartupInput,
    StartupInputs, account_startup, candidate_departed, candidate_returned,
    handle_candidate_packet, is_startup_receipt, next_timed_room_event, publish_startup_failure,
    ready_runtime_activity, resend_startup_readiness, startup_output_limit, startup_ready_notice,
    startup_state,
};

#[path = "startup_publication.rs"]
pub(crate) mod publication;

#[path = "startup_open.rs"]
mod opening;

#[path = "startup_media.rs"]
mod input;
use input::{startup_deadline_backpressure, startup_with_audio_until_output};

pub(crate) async fn publish_startup_readiness(
    gemini: &mut GeminiLiveSession,
    usage: &mut crate::gemini::TokenUsage,
    publication: impl Future<Output = Result<(), Box<dyn std::error::Error + Send + Sync>>>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let (_tx, receiver) = mpsc::unbounded_channel();
    super::super::readiness::publish_startup_readiness(
        gemini,
        usage,
        &tokio::sync::Mutex::new(receiver.into()),
        &mut Vec::new(),
        &mut false,
        publication,
    )
    .await
}

async fn probe_live_output(
    gemini: &mut GeminiLiveSession,
    state: &mut RuntimeState,
    greeting: &str,
    limit: Duration,
) -> Result<Instant, Box<dyn std::error::Error + Send + Sync>> {
    let (_tx, mut input) = mpsc::unbounded_channel();
    super::super::readiness::probe_live_output(
        gemini,
        state,
        greeting,
        limit,
        &mut StartupContext {
            media: &mut CandidateMedia::new(),
            candidate_identity: "candidate",
            candidate_video: false,
            deferred_events: &mut Vec::new(),
            room_closed: &mut false,
        },
        &mut input,
    )
    .await
}

impl StartupInputs for mpsc::UnboundedReceiver<StartupInput> {
    async fn next(&mut self, _media: &mut CandidateMedia) -> StartupInput {
        self.recv().await.unwrap()
    }
}

/// A room loop as the interview starts it, far from its hard deadline.
pub(crate) fn room_loop(restarts: usize) -> RoomLoop {
    RoomLoop {
        pending_round_notice: None,
        deadline_at: tokio::time::Instant::now() + Duration::from_secs(3600),
        recovery_terminal: None,
        recovery_inputs: None,
        recovery_phase: None,
        recovery_reader: None,
        recovery_writes: Default::default(),
        recovery_after: Default::default(),
        restarts,
        deferred_restart: DeferredRestart::default(),
        resume_recovery: ResumeRecovery::default(),
        interim_review: SideCall::default(),
        presence: CandidatePresence::default(),
    }
}

// A failed protocol assertion must cancel its peer rather than detach a task
// parked on a WebSocket read. Keep JoinHandle's usual await result.
pub(crate) struct StartupPeer<T>(tokio::task::JoinHandle<T>);

impl<T> Future for StartupPeer<T> {
    type Output = Result<T, tokio::task::JoinError>;

    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        std::pin::Pin::new(&mut self.0).poll(cx)
    }
}

impl<T> Drop for StartupPeer<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}

// Wall time bounds fixture IO even while Tokio time is deliberately paused.
// Yielding keeps the virtual clock from auto-advancing across a tested cutoff.
async fn startup_fixture_io<T>(phase: &str, future: impl Future<Output = T>) -> T {
    use futures_util::FutureExt;
    tokio::pin!(future);
    let safety = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(result) = future.as_mut().now_or_never() {
            return result;
        }
        assert!(Instant::now() < safety, "startup fixture stalled: {phase}");
        tokio::task::yield_now().await;
    }
}

struct StartupWire {
    audio: Vec<u8>,
    generations: usize,
    pings: usize,
}

async fn startup_socket(
    answer_after_bytes: Option<usize>,
) -> (
    GeminiLiveSession,
    StartupPeer<StartupWire>,
    Arc<AtomicUsize>,
) {
    let config = startup_config("60");
    let keys = GeminiKeys::from_config(&config);
    let boot = crate::runtime::bootstrap(&config, "interview-fixed", Some("two-sum"), 45);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}/", listener.local_addr().unwrap());
    let received = Arc::new(AtomicUsize::new(0));
    let observed = std::sync::Arc::clone(&received);
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
                    if value["clientContent"]["turnComplete"] == true
                        || value["realtimeInput"]["text"].is_string()
                    {
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
    (gemini, StartupPeer(server), received)
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

// A real local Gemini socket, with output released explicitly by the test.
// Every generating request is counted, including any accidental replacement.
async fn reconnect_socket(
    boot: &RuntimeBootstrap<'_>,
) -> (
    GeminiLiveSession,
    tokio::sync::oneshot::Receiver<()>,
    tokio::sync::oneshot::Sender<bool>,
    StartupPeer<usize>,
) {
    let config = startup_config("60");
    let keys = GeminiKeys::from_config(&config);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}/", listener.local_addr().unwrap());
    let (asked, requested) = tokio::sync::oneshot::channel();
    let (release, output) = tokio::sync::oneshot::channel();
    let candidate_led = boot.candidate_led_enabled;
    let server = tokio::spawn(async move {
        use futures_util::FutureExt;
        let (socket, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
        let setup = socket.next().await.unwrap().unwrap();
        let setup: serde_json::Value = serde_json::from_str(setup.to_text().unwrap()).unwrap();
        assert_eq!(
            setup["setup"]["systemInstruction"]["parts"][0]["text"]
                .as_str()
                .unwrap()
                .contains("CANDIDATE-LED CODING"),
            candidate_led
        );
        socket
            .send(Message::Text(r#"{"setupComplete":{}}"#.into()))
            .await
            .unwrap();
        let mut generation = 0;
        let mut asked = Some(asked);
        let mut output = Some(output);
        while let Some(Ok(message)) = socket.next().await {
            if message.is_close() {
                break;
            }
            if let Message::Text(text) = message {
                let value: serde_json::Value = serde_json::from_str(&text).unwrap();
                if value["clientContent"]["turnComplete"] == true
                    || value["realtimeInput"]["text"].is_string()
                {
                    generation += 1;
                    if let Some(asked) = asked.take() {
                        asked.send(()).unwrap();
                        let frame = if output.take().unwrap().await.unwrap() {
                            r#"{"serverContent":{"modelTurn":{"parts":[{"text":"Hello"}]},"turnComplete":true}}"#
                        } else {
                            r#"{"error":{"code":429,"message":"resource exhausted"}}"#
                        };
                        socket.send(Message::Text(frame.into())).await.unwrap();
                    }
                }
            }
        }
        assert!(
            listener.accept().now_or_never().is_none(),
            "no replacement socket"
        );
        generation
    });
    let gemini = crate::gemini::live_session_with_keys_at(&url, &keys, boot, None)
        .await
        .unwrap();
    (gemini, requested, release, StartupPeer(server))
}

#[tokio::test]
async fn startup_reconnect_synchronizes_the_paid_opening_without_regeneration() {
    use futures_util::FutureExt;
    for (query_first, candidate_led) in [(false, false), (true, false), (false, true), (true, true)]
    {
        let config = startup_config("60");
        let keys = Arc::new(GeminiKeys::from_config(&config));
        let metadata = serde_json::json!({"candidateLed": candidate_led}).to_string();
        let boot = candidate_bootstrap(&config, "interview-fixed", Some(&metadata));
        assert_eq!(boot.candidate_led_enabled, candidate_led);
        let (mut gemini, requested, release, server) = reconnect_socket(&boot).await;
        let mut turn = hold_turn(initial_runtime_state(&boot, Instant::now()));
        assert_eq!(turn.state.candidate_led_enabled, candidate_led);
        let mut presence = CandidatePresence::default();
        let greeting = with_timer(&turn.state, "opening".to_string());
        let requested_at = Instant::now();
        let started_at = {
            let probe = probe_live_output(
                &mut gemini,
                &mut turn.state,
                &greeting,
                Duration::from_secs(5),
            );
            tokio::pin!(probe);
            tokio::select! {
                result = &mut probe => panic!("setup is not readiness: {result:?}"),
                _ = requested => {}
            }
            // The candidate leaves while generation is pending, before output.
            presence.left(Instant::now());
            release.send(true).unwrap();
            probe.await.unwrap()
        };
        turn.state.started_at = started_at;
        turn.activity = ready_runtime_activity(
            &config,
            boot.room_name,
            started_at,
            requested_at,
            7,
            &greeting,
        );
        let original_clock = turn.activity.startup_ready_at.unwrap();
        let original_prompt = turn.activity.prompt_sequence;
        let (room, sent) = RoomLink::recording(boot.room_name);
        let ready = startup_ready_notice(started_at, Some(original_clock));
        let (release_notice, notice_ready) = tokio::sync::oneshot::channel();
        let (return_candidate, returned) = tokio::sync::oneshot::channel();
        {
            let mut usage = Default::default();
            let publishing = publish_startup_readiness(&mut gemini, &mut usage, async {
                notice_ready.await.unwrap();
                publish_interviewer_state(&room, &ready).await
            });
            tokio::pin!(publishing);
            assert!(publishing.as_mut().now_or_never().is_none());

            // Return arrives while publication is pending. Like the room loop,
            // apply queued presence/control events only after startup returns.
            return_candidate.send("candidate").unwrap();
            release_notice.send(()).unwrap();
            publishing.await.unwrap();
        }
        // Nothing in reliable data retains this packet for the absent browser.
        assert_eq!(
            sent.take(),
            vec![room_link::Sent::Data {
                topic: Some(TOPIC_CONTROL.into()),
                message: ready.clone()
            }]
        );
        assert_eq!(sent.take_data_destinations(), vec![Vec::<String>::new()]);
        let (_tx, events) = mpsc::unbounded_channel();
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
        let (mut audio, _frames) = test_output_audio();
        let mut board = Board::new();
        let mut media = CandidateMedia::new();
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../../fixtures/control.json")).unwrap();
        let query = fixture["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|case| case["name"] == "startup sync")
            .unwrap()["payload"]
            .clone();
        assert!(
            interview_packet(
                Some(TOPIC_CONTROL),
                Some("bystander"),
                "candidate",
                &serde_json::to_vec(&query).unwrap()
            )
            .is_none()
        );
        let (topic, query) = interview_packet(
            Some(TOPIC_CONTROL),
            Some("candidate"),
            "candidate",
            &serde_json::to_vec(&query).unwrap(),
        )
        .unwrap();
        {
            let mut context = turn.context(&mut audio, &mut gemini, &mut board, &mut media);

            // Query versus return can land in either order. Both use the same
            // state; neither needs the buffered opening to be replayed first.
            if query_first {
                assert!(
                    handle_candidate_packet(
                        &room,
                        &mut context,
                        &presence,
                        interview,
                        topic,
                        &query
                    )
                    .await
                    .unwrap()
                    .is_continue()
                );
            }
            candidate_returned(
                &room,
                &mut context,
                &mut presence,
                returned.await.unwrap(),
                Instant::now(),
            )
            .await;
            assert!(
                handle_candidate_packet(&room, &mut context, &presence, interview, topic, &query)
                    .await
                    .unwrap()
                    .is_continue()
            );
        }
        let notices = sent.take();
        assert_eq!(notices.len(), if query_first { 3 } else { 2 });
        for notice in notices {
            let room_link::Sent::Data { topic, mut message } = notice else {
                panic!("readiness must use the control topic");
            };
            assert_eq!(topic.as_deref(), Some(TOPIC_CONTROL));
            assert!(message["elapsedMs"].as_u64().unwrap() >= ready["elapsedMs"].as_u64().unwrap());
            message["elapsedMs"] = ready["elapsedMs"].clone();
            assert_eq!(message, ready);
        }
        let destinations = sent.take_data_destinations();
        assert_eq!(destinations.len(), if query_first { 3 } else { 2 });
        for destinations in destinations {
            assert_eq!(destinations, vec!["candidate"]);
        }
        assert_eq!(presence.left_at, None);
        assert_eq!(turn.state.started_at, started_at);
        assert_eq!(turn.activity.startup_ready_at, Some(original_clock));
        assert_eq!(turn.activity.buffered_startup_output_at, Some(started_at));
        assert_eq!(turn.activity.prompt_sequence, original_prompt);
        assert_eq!(turn.activity.live_socket, 1);
        assert_eq!(turn.state.candidate_led_enabled, candidate_led);
        assert_eq!(
            gemini.next_event().await,
            Some(GeminiEvent::Text("Hello".into()))
        );
        assert_eq!(gemini.event_received_at(), Some(started_at));
        assert_eq!(gemini.next_event().await, Some(GeminiEvent::TurnComplete));
        hand_over(&mut turn.state, &mut turn.activity, &mut audio);
        assert_eq!(turn.state.candidate_led_enabled, candidate_led);
        assert_eq!(turn.activity.startup_ready_at, Some(original_clock));
        let mut context = turn.context(&mut audio, &mut gemini, &mut board, &mut media);
        assert!(
            handle_candidate_packet(&room, &mut context, &presence, interview, topic, &query)
                .await
                .unwrap()
                .is_continue()
        );
        let notices = sent.take();
        assert_eq!(notices.len(), 1);
        let room_link::Sent::Data { topic, mut message } = notices.into_iter().next().unwrap()
        else {
            panic!("readiness must use the control topic");
        };
        assert_eq!(topic.as_deref(), Some(TOPIC_CONTROL));
        assert!(message["elapsedMs"].as_u64().unwrap() >= ready["elapsedMs"].as_u64().unwrap());
        let age_after = u64::try_from(started_at.elapsed().as_millis()).unwrap();
        assert!(message["elapsedMs"].as_u64().unwrap() <= age_after);
        message["elapsedMs"] = ready["elapsedMs"].clone();
        assert_eq!(message, ready);
        gemini.shutdown().await.unwrap();
        drop(gemini);
        assert_eq!(server.await.unwrap(), 1);
    }
}

#[test]
fn startup_readiness_age_is_independent_of_the_server_wall_clock() {
    let started_at = Instant::now() - Duration::from_secs(10);
    for wall_clock in [1, u64::MAX] {
        let notice = startup_ready_notice(started_at, Some(wall_clock));
        assert_eq!(notice["startedAt"], wall_clock);
        assert!((10_000..11_000).contains(&notice["elapsedMs"].as_u64().unwrap()));
    }
}

#[tokio::test]
async fn startup_reconnect_does_not_revive_failed_ended_or_expired_interviews() {
    for terminal in ["failed", "ended", "expired"] {
        let config = startup_config("60");
        let keys = Arc::new(GeminiKeys::from_config(&config));
        let boot = crate::runtime::bootstrap(&config, "interview-fixed", Some("two-sum"), 45);
        let (mut gemini, requested, release, server) = reconnect_socket(&boot).await;
        let mut turn = hold_turn(initial_runtime_state(&boot, Instant::now()));
        let greeting = with_timer(&turn.state, "opening".to_string());
        let probed = {
            let probe = probe_live_output(
                &mut gemini,
                &mut turn.state,
                &greeting,
                Duration::from_secs(5),
            );
            tokio::pin!(probe);
            tokio::select! { result = &mut probe => panic!("before output: {result:?}"), _ = requested => {} }
            release.send(terminal != "failed").unwrap();
            probe.await
        };
        let start = Instant::now();
        if terminal == "failed" {
            assert_eq!(
                crate::gemini::startup_failure_reason(probed.unwrap_err().as_ref()),
                "quota"
            );
        } else {
            turn.state.started_at = probed.unwrap();
            turn.activity = ready_runtime_activity(
                &config,
                boot.room_name,
                turn.state.started_at,
                start,
                7,
                &greeting,
            );
            turn.state.ended = terminal == "ended";
        }
        let (room, sent) = RoomLink::recording(boot.room_name);
        let mut presence = CandidatePresence::default();
        presence.left(if terminal == "expired" {
            start - CANDIDATE_ABSENCE_LIMIT - Duration::from_secs(1)
        } else {
            start
        });
        let (_tx, events) = mpsc::unbounded_channel();
        let events = tokio::sync::Mutex::new(RoomEventReader::from(events));
        let interview = InterviewContext {
            config: &config,
            keys: &keys,
            boot: &boot,
            live_endpoint: None,
            started_at: start,
            candidate_identity: "candidate",
            events: &events,
            slot: None,
        };
        let (mut audio, _frames) = test_output_audio();
        let mut board = Board::new();
        let mut media = CandidateMedia::new();
        let mut context = turn.context(&mut audio, &mut gemini, &mut board, &mut media);
        candidate_returned(
            &room,
            &mut context,
            &mut presence,
            "candidate",
            Instant::now(),
        )
        .await;
        assert!(
            handle_candidate_packet(
                &room,
                &mut context,
                &presence,
                interview,
                TOPIC_CONTROL,
                &serde_json::json!({"type": "startup_sync"})
            )
            .await
            .unwrap()
            .is_continue()
        );
        assert!(
            sent.take().is_empty(),
            "{terminal} must never publish ready"
        );
        if terminal == "expired" {
            assert!(presence.gave_up(start));
            let mut loops = RoomLoop {
                presence,
                ..room_loop(0)
            };
            assert!(
                on_watch_tick(&room, &mut context, &mut loops, interview, start)
                    .await
                    .unwrap()
                    .is_break()
            );
            assert_eq!(sent.take(), vec![room_link::Sent::Left]);
        }
        gemini.shutdown().await.unwrap();
        drop(gemini);
        assert_eq!(server.await.unwrap(), 1);
    }
}

#[test]
fn startup_ready_publication_budget_leaves_browser_cleanup_headroom() {
    let browser = include_str!("../../../web/lib.js");
    let timeout_ms: u64 = browser
        .split("export function createInterviewerReadiness({")
        .nth(1)
        .unwrap()
        .split("timeoutMs = ")
        .nth(1)
        .unwrap()
        .split(',')
        .next()
        .unwrap()
        .replace('_', "")
        .parse()
        .unwrap();
    let mut config = startup_config("120");
    config.gemini_reply_timeout_s = crate::config::MAX_GEMINI_REPLY_TIMEOUT_S;
    config.gemini_first_output_s = crate::config::MAX_GEMINI_REPLY_TIMEOUT_S;
    let bounded_startup = crate::gemini::FIRST_OPEN_LIMIT
        + startup_output_limit(&config)
        + STARTUP_READY_PUBLICATION_LIMIT
        + crate::gemini::CLOSE_TIMEOUT
        + RECONNECT_NOTICE_LIMIT;
    assert_eq!(startup_output_limit(&config), Duration::from_secs(120));
    assert!(bounded_startup + Duration::from_secs(10) < Duration::from_millis(timeout_ms));
    assert!(STARTUP_READY_PUBLICATION_LIMIT > RECONNECT_NOTICE_LIMIT);
}

#[test]
fn startup_deferred_departure_keeps_its_original_grace_deadline() {
    let observed = Instant::now() - Duration::from_secs(80);
    let mut presence = CandidatePresence::default();
    candidate_departed(&mut presence, observed);
    assert_eq!(
        presence.deadline(CANDIDATE_ABSENCE_LIMIT),
        Some(observed + CANDIDATE_ABSENCE_LIMIT)
    );
    assert!(!presence.gave_up(observed + CANDIDATE_ABSENCE_LIMIT - Duration::from_nanos(1)));
    candidate_departed(&mut presence, observed + Duration::from_secs(20));
    assert!(
        presence.gave_up(observed + CANDIDATE_ABSENCE_LIMIT),
        "a duplicate deferred departure cannot buy more grace"
    );
}

#[test]
fn startup_deferred_presence_is_checked_without_waiting_for_other_queued_work() {
    let now = Instant::now();
    let observed = now - Duration::from_secs(100);
    for returned in [None, Some(89), Some(90), Some(91)] {
        let current = CandidatePresence::default();
        let changes = [
            Some(CandidatePresenceChange::Left(observed)),
            returned.map(|seconds| {
                CandidatePresenceChange::Returned(observed + Duration::from_secs(seconds))
            }),
        ];
        let projected = current.projected(changes.into_iter().flatten());
        assert_eq!(projected.gave_up(now), returned != Some(89));
        assert_eq!(
            current.left_at, None,
            "the watch must not apply queued side effects early"
        );
    }
    let current = CandidatePresence {
        left_at: Some(observed),
        ..CandidatePresence::default()
    };
    let projected = current.projected(
        [
            CandidatePresenceChange::Returned(observed + Duration::from_secs(89)),
            CandidatePresenceChange::Left(now - Duration::from_secs(5)),
        ]
        .into_iter(),
    );
    assert!(!projected.gave_up(now));
    assert_eq!(
        projected.deadline(CANDIDATE_ABSENCE_LIMIT),
        Some(now + Duration::from_secs(85))
    );
}

#[tokio::test]
async fn startup_deferred_return_is_judged_when_observed_not_when_replayed() {
    for elapsed in [89, 90, 91] {
        let (mut gemini, server, _) = startup_socket(None).await;
        let config = startup_config("60");
        let mut turn = hold_turn(startup_state_for(&config));
        let (room, sent) = RoomLink::recording("presence-test");
        let (mut audio, _frames) = test_output_audio();
        let mut board = Board::new();
        let mut media = CandidateMedia::new();
        let observed = Instant::now() - Duration::from_secs(100);
        let mut presence = CandidatePresence::default();
        presence.left(observed);
        let mut context = turn.context(&mut audio, &mut gemini, &mut board, &mut media);
        candidate_returned(
            &room,
            &mut context,
            &mut presence,
            "candidate",
            observed + Duration::from_secs(elapsed),
        )
        .await;
        assert_eq!(
            presence.left_at.is_none(),
            elapsed < 90,
            "return at {elapsed}s replayed after 100s"
        );
        if elapsed < 90 {
            let second_departure = Instant::now() - Duration::from_secs(5);
            candidate_departed(&mut presence, second_departure);
            assert_eq!(
                presence.left_at,
                Some(second_departure),
                "a new absence after a timely return has its own original deadline"
            );
        }
        assert!(
            sent.take().is_empty(),
            "this fixture has no successful opening to replay"
        );
        gemini.shutdown().await.unwrap();
        assert_eq!(server.await.unwrap().generations, 0);
    }
}

#[tokio::test]
async fn startup_pumps_more_than_the_audio_queue_before_a_late_first_output() {
    let wire = startup_with_audio_until_output("60", 5_500).await;
    assert!((3..=5).contains(&wire.pings), "pings={}", wire.pings);
}

#[tokio::test]
async fn startup_outer_deadline_preserves_output_received_during_microphone_backpressure() {
    startup_deadline_backpressure(
        Duration::from_secs(10),
        Some(Duration::from_secs(9)),
        true,
        true,
    )
    .await;
}

#[tokio::test]
async fn startup_outer_deadline_rejects_absent_late_and_nonqualifying_output() {
    for (answer_at, output) in [
        (None, false),
        (Some(Duration::from_millis(1100)), true),
        (Some(Duration::from_millis(900)), false),
    ] {
        startup_deadline_backpressure(Duration::from_secs(1), answer_at, output, true).await;
    }
}

#[tokio::test]
async fn startup_outer_deadline_rechecks_late_inner_success() {
    startup_deadline_backpressure(
        Duration::from_secs(1),
        Some(Duration::from_millis(1100)),
        true,
        false,
    )
    .await;
}

#[tokio::test]
async fn startup_deadline_settlement_preserves_the_room_error() {
    use futures_util::FutureExt;
    let (mut gemini, server, requested) = startup_socket(None).await;
    let config = startup_config("60");
    let mut state = startup_state_for(&config);
    let mut media = CandidateMedia::new();
    let mut deferred = Vec::new();
    let mut room_closed = false;
    let (tx, mut input) = mpsc::unbounded_channel();
    tokio::time::pause();
    let limit = Duration::from_secs(1);
    let deadline = tokio::time::Instant::now() + limit;
    let error = {
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
            "opening",
            limit,
            &mut context,
            &mut input,
        );
        tokio::pin!(probe);
        startup_fixture_io("opening request", async {
            loop {
                assert!(probe.as_mut().now_or_never().is_none());
                if requested.load(Ordering::SeqCst) != 0 {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await;
        tokio::time::advance(limit).await;
        assert_eq!(tokio::time::Instant::now(), deadline);
        tx.send(StartupInput::Room(None)).unwrap();
        probe.await.unwrap_err()
    };
    tokio::time::resume();
    assert!(room_closed);
    assert_eq!(
        error.downcast_ref::<std::io::Error>().unwrap().kind(),
        std::io::ErrorKind::Interrupted
    );
    assert_eq!(error.to_string(), "room closed before startup output");
    gemini.shutdown().await.unwrap();
    assert_eq!(
        startup_fixture_io("peer close", server)
            .await
            .unwrap()
            .generations,
        1
    );
}

#[tokio::test]
async fn startup_probe_rejects_output_received_exactly_at_its_deadline() {
    use futures_util::FutureExt;
    let (mut gemini, server, requested) = startup_socket(None).await;

    // Control the reader-to-probe queue, not the probe or rescue result. An
    // observation at the cutoff must pass through wait_for_model_output first.
    let output = gemini.startup_test_events();
    let config = startup_config("60");
    let mut state = startup_state_for(&config);
    tokio::time::pause();
    let limit = Duration::from_secs(1);
    let deadline = tokio::time::Instant::now() + limit;
    let result = {
        let probe = probe_live_output(&mut gemini, &mut state, "opening", limit);
        tokio::pin!(probe);
        startup_fixture_io("opening request", async {
            loop {
                assert!(probe.as_mut().now_or_never().is_none());
                if requested.load(Ordering::SeqCst) != 0 {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await;
        tokio::time::advance(limit).await;
        assert_eq!(tokio::time::Instant::now(), deadline);
        output
            .send((deadline.into_std(), GeminiEvent::Text("Hello".into())))
            .await
            .unwrap();
        probe.await
    };
    tokio::time::resume();
    let error = result.expect_err("deadline equality is expired, including the normal output arm");
    assert_eq!(
        error.downcast_ref::<std::io::Error>().unwrap().kind(),
        std::io::ErrorKind::TimedOut
    );
    assert_eq!(
        startup_fixture_io("cutoff output replay", gemini.next_event()).await,
        Some(GeminiEvent::Text("Hello".into()))
    );
    assert_eq!(gemini.event_received_at(), Some(deadline.into_std()));
    gemini.shutdown().await.unwrap();
    assert_eq!(
        startup_fixture_io("peer close", server)
            .await
            .unwrap()
            .generations,
        1
    );
}

#[test]
fn startup_delivery_error_exposes_its_typed_source() {
    let error = StartupDeliveryError(
        std::io::Error::new(std::io::ErrorKind::BrokenPipe, "ready data channel closed").into(),
    );
    let source = std::error::Error::source(&error).expect("delivery failure retains its source");
    let source = source
        .downcast_ref::<std::io::Error>()
        .expect("underlying IO error type");
    assert_eq!(source.kind(), std::io::ErrorKind::BrokenPipe);
    assert_eq!(source.to_string(), "ready data channel closed");
    assert_eq!(startup_state(Some(&error))["reason"], "delivery");
}

#[tokio::test]
async fn startup_sync_routing_leaves_thinking_and_end_to_the_runtime() {
    use futures_util::FutureExt;
    let config = startup_config("60");
    let keys = Arc::new(GeminiKeys::single(""));
    let boot = crate::runtime::bootstrap(&config, "packet-routing", Some("two-sum"), 45);
    let (mut gemini, server, _) = startup_socket(None).await;
    let mut turn = hold_turn(initial_runtime_state(&boot, Instant::now()));
    let (room, sent) = RoomLink::recording(boot.room_name);
    let (_events, events) = mpsc::unbounded_channel();
    let events = tokio::sync::Mutex::new(RoomEventReader::from(events));
    let interview = InterviewContext {
        config: &config,
        keys: &keys,
        boot: &boot,
        live_endpoint: None,
        started_at: turn.state.started_at,
        candidate_identity: "candidate",
        events: &events,
        slot: None,
    };
    let (mut audio, _frames) = test_output_audio();
    let mut board = Board::new();
    let mut media = CandidateMedia::new();
    let presence = CandidatePresence::default();
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("../../fixtures/control.json")).unwrap();
    let command = |name| {
        fixture["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|case| case["name"] == name)
            .unwrap()["payload"]
            .clone()
    };
    assert!(!turn.state.interviewer_silenced());
    {
        let mut context = turn.context(&mut audio, &mut gemini, &mut board, &mut media);
        assert!(
            startup_fixture_io(
                "thinking handler",
                handle_candidate_packet(
                    &room,
                    &mut context,
                    &presence,
                    interview,
                    TOPIC_CONTROL,
                    &command("thinking start"),
                )
            )
            .await
            .unwrap()
            .is_continue()
        );
    }
    assert!(
        turn.state.interviewer_silenced(),
        "Thinking reached the runtime reducer"
    );
    assert!(!turn.state.ended);
    {
        let mut context = turn.context(&mut audio, &mut gemini, &mut board, &mut media);
        let end = command("end interview");

        // Hold the report-recovery receiver so this routing check stops after
        // reduction/freeze and before farewell or report delivery starts. A
        // recorded room does not implement the latter's live receipt path.
        let _receiver = events.lock().await;
        let ending = handle_candidate_packet(
            &room,
            &mut context,
            &presence,
            interview,
            TOPIC_CONTROL,
            &end,
        );
        tokio::pin!(ending);
        assert!(ending.as_mut().now_or_never().is_none());
    }
    assert!(turn.state.ended, "End reached the runtime reducer");
    assert!(
        sent.take().iter().all(|event| !matches!(event,
            room_link::Sent::Data { message, .. } if message["startup"] == "ready"
        )),
        "ordinary controls are not startup sync requests"
    );
    gemini.shutdown().await.unwrap();
    startup_fixture_io("routing peer close", server)
        .await
        .unwrap();
}

#[tokio::test]
async fn startup_resend_diagnostics_match_the_delivery_result() {
    const CHILD: &str = "CODETRIAL_STARTUP_RESEND_DIAGNOSTIC_CHILD";
    if let Ok(outcome) = std::env::var(CHILD) {
        let config = startup_config("60");
        let boot = crate::runtime::bootstrap(&config, "presence-test", Some("two-sum"), 45);
        let (mut gemini, server, _) = startup_socket(None).await;
        let mut turn = hold_turn(initial_runtime_state(&boot, Instant::now()));
        turn.activity.startup_ready_at = Some(7);
        let (recorded, sent) = RoomLink::recording(boot.room_name);
        let room = if outcome == "error" {
            let (room, _events, _arrive, peer) =
                publication::candidate_room("candidate", "{}", true).await;
            startup_fixture_io("closed diagnostic room", room.close())
                .await
                .unwrap();
            startup_fixture_io("diagnostic signal peer close", peer)
                .await
                .unwrap();
            RoomLink::live(room)
        } else {
            recorded
        };
        let (mut audio, _frames) = test_output_audio();
        let mut board = Board::new();
        let mut media = CandidateMedia::new();
        let context = turn.context(&mut audio, &mut gemini, &mut board, &mut media);
        startup_fixture_io(
            "resend result",
            resend_startup_readiness(&room, &context, &CandidatePresence::default(), "candidate"),
        )
        .await;
        if outcome == "success" {
            assert_eq!(sent.take().len(), 1);
            assert_eq!(
                sent.take_data_destinations(),
                vec![vec!["candidate".to_string()]]
            );
        }
        gemini.shutdown().await.unwrap();
        startup_fixture_io("diagnostic Gemini close", server)
            .await
            .unwrap();
        return;
    }
    for outcome in ["success", "error"] {
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "livekit::tests::startup::startup_resend_diagnostics_match_the_delivery_result",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(CHILD, outcome)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(15);
        while child.try_wait().unwrap().is_none() {
            if Instant::now() >= deadline {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("diagnostic child stalled: {outcome}");
            }
            tokio::task::yield_now().await;
        }
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(
            stderr
                .lines()
                .filter(|line| *line == "startup state resync failed; room=presence-test")
                .count(),
            usize::from(outcome == "error"),
            "{outcome}: {stderr}"
        );
    }
}

#[test]
fn output_settles_both_startup_progress_and_the_handoff_reply_wait() {
    let start = Instant::now();
    let mut activity = RuntimeActivity::new(start);
    activity.buffered_startup_output_at = Some(start);
    activity.arm_handoff_reply(start);
    let silence = Duration::from_millis(1_500);
    let due = start + silence + HANDOFF_REPLY_MARGIN;
    assert!(activity.handoff_reply_due(due, silence, false));

    activity.note_output(start + Duration::from_millis(1));
    assert!(activity.buffered_startup_output_at.is_none());
    assert!(!activity.handoff_reply_due(due, silence, false));
}
