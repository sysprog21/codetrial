use super::super::tests::{fake_recording_socket, hold_turn, test_output_audio};
use super::*;
use crate::config::load_from_pairs;
use futures_util::{SinkExt, StreamExt};
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::tungstenite::Message;

fn isolated(name: &str) -> bool {
    isolated_checked(name, |_| {})
}

fn isolated_checked(name: &str, check: impl FnOnce(&std::process::Output)) -> bool {
    isolated_checked_with_loopback(name, false, check)
}

fn isolated_checked_with_loopback(
    name: &str,
    loopback_http: bool,
    check: impl FnOnce(&std::process::Output),
) -> bool {
    let name = format!("livekit::recovery::tests::{name}");
    if std::env::var("CODETRIAL_RECOVERY_TEST").as_deref() == Ok(&name) {
        return false;
    }
    let guard = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    guard.set_nonblocking(true).unwrap();
    let proxy = format!("http://{}", guard.local_addr().unwrap());
    let mut command = std::process::Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", &name, "--nocapture"])
        .env("CODETRIAL_RECOVERY_TEST", &name)
        .env("NO_PROXY", if loopback_http { "127.0.0.1" } else { "" })
        .env("no_proxy", if loopback_http { "127.0.0.1" } else { "" })
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    for key in [
        "HTTPS_PROXY",
        "https_proxy",
        "HTTP_PROXY",
        "http_proxy",
        "ALL_PROXY",
        "all_proxy",
    ] {
        command.env(key, &proxy);
    }
    let mut child = command.spawn().unwrap();

    // Drained while the child runs: a child that writes more than a pipe holds
    // (64 KiB on Linux) blocks in its write, and polling only for its exit then
    // reads that block as a hang.
    let drain = |stream: Option<Box<dyn std::io::Read + Send>>| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            if let Some(mut stream) = stream {
                let _ = stream.read_to_end(&mut bytes);
            }
            bytes
        })
    };
    let stdout = drain(
        child
            .stdout
            .take()
            .map(|pipe| Box::new(pipe) as Box<dyn std::io::Read + Send>),
    );
    let stderr = drain(
        child
            .stderr
            .take()
            .map(|pipe| Box::new(pipe) as Box<dyn std::io::Read + Send>),
    );
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if started.elapsed() > Duration::from_secs(20) {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!(
                "recovery child timed out: {}",
                String::from_utf8_lossy(&stderr.join().unwrap())
            );
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let output = std::process::Output {
        status,
        stdout: stdout.join().unwrap(),
        stderr: stderr.join().unwrap(),
    };
    assert_eq!(
        guard.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock,
        "the recovery test must not contact an external provider"
    );
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.lines().any(|line| line == "running 1 test")
            && stdout.lines().any(|line| {
                line.starts_with("test result: ok. 1 passed; 0 failed; 0 ignored;")
            }),
        "isolated child must execute exactly one passing test: {name}\n{stdout}"
    );
    check(&output);
    true
}

#[test]
fn isolation_rejects_a_name_that_selects_no_test() {
    let failure = std::panic::catch_unwind(|| isolated("deliberately_missing_recovery_test"))
        .expect_err("a successful zero-test child must not qualify a recovery regression");
    let message = failure
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| failure.downcast_ref::<&str>().copied())
        .expect("the isolation guard must explain the rejected test count");
    assert!(
        message.contains("isolated child must execute exactly one passing test"),
        "an unrelated child failure is not a test-count regression: {message}"
    );
}

struct Rig {
    config: AgentConfig,
    keys: Arc<GeminiKeys>,
    turn: TurnState,
    gemini: GeminiLiveSession,
    old: Option<tokio::task::JoinHandle<Vec<serde_json::Value>>>,
    expected_old_messages: usize,
    prefix: bool,
    speech: Option<session::SpeechHandoff>,
    output: OutputAudio,
    _frames: mpsc::Receiver<QueuedOutputFrame>,
    board: Board,
    media: CandidateMedia,
    room: RoomLink,
    recorder: Arc<room_link::Recorder>,
    loops: RoomLoop,
    events: tokio::sync::Mutex<mpsc::UnboundedReceiver<RoomEvent>>,
    tx: mpsc::UnboundedSender<Packet>,
    _room_sender: mpsc::UnboundedSender<RoomEvent>,
    terminal: mpsc::UnboundedReceiver<Terminal>,
    endpoint: String,
}

impl Rig {
    async fn new(endpoint: String) -> Self {
        Self::with_old_usage(endpoint, None).await
    }

    async fn with_old_usage(endpoint: String, old_usage: Option<u64>) -> Self {
        Self::with_old_checkpoint(endpoint, old_usage, false).await
    }

    async fn with_old_checkpoint(
        endpoint: String,
        old_usage: Option<u64>,
        checkpoint: bool,
    ) -> Self {
        Self::with_old_resumption(
            endpoint,
            old_usage,
            checkpoint
                .then(|| serde_json::json!({"resumable": true, "newHandle": "matrix-checkpoint"})),
        )
        .await
    }

    async fn with_old_resumption(
        endpoint: String,
        old_usage: Option<u64>,
        update: Option<serde_json::Value>,
    ) -> Self {
        let config = load_from_pairs([
            ("LIVEKIT_URL", "wss://example.livekit.cloud"),
            ("LIVEKIT_API_KEY", "devkey"),
            ("LIVEKIT_API_SECRET", "devsecret"),
            ("GOOGLE_API_KEY", "sequence-test-key"),
        ])
        .unwrap();
        let keys = Arc::new(GeminiKeys::from_config(&config));
        let (old_listener, old_endpoint) = listener().await;
        let old = tokio::spawn(async move {
            use tokio_tungstenite::tungstenite::{Error as WsError, error::ProtocolError};
            let (socket, _) = old_listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
            socket.next().await.unwrap().unwrap();
            socket
                .send(Message::Text(r#"{"setupComplete":{}}"#.into()))
                .await
                .unwrap();
            if let Some(prompt) = old_usage {
                let mut observation = serde_json::json!({
                    "usageMetadata": {"promptTokenCount": prompt}
                });
                if let Some(update) = &update {
                    observation["sessionResumptionUpdate"] = update.clone();
                }
                socket
                    .send(Message::Text(observation.to_string().into()))
                    .await
                    .unwrap();
            }
            let mut messages = Vec::new();
            while let Some(frame) = socket.next().await {
                match frame {
                    Ok(frame) if frame.is_close() => break,
                    Ok(frame) => {
                        messages.push(serde_json::from_str(frame.to_text().unwrap()).unwrap());
                    }
                    Err(WsError::Protocol(ProtocolError::ResetWithoutClosingHandshake)) => break,
                    Err(WsError::Io(error))
                        if matches!(
                            error.kind(),
                            std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::UnexpectedEof
                        ) =>
                    {
                        break;
                    }
                    Err(error) => panic!("unexpected retired peer error: {error}"),
                }
            }
            messages
        });
        let boot = crate::runtime::bootstrap(&config, "recovery-test", Some("two-sum"), 45);
        let gemini = crate::livekit::tests::fixture_session_at(&old_endpoint, &keys, &boot, None)
            .await
            .unwrap();
        let (output, frames) = test_output_audio();
        let (room, recorder) = room_link::RoomLink::recording("recovery-test");
        let (room_sender, events) = mpsc::unbounded_channel();
        let (tx, inputs) = mpsc::unbounded_channel();
        let (probe, terminal) = mpsc::unbounded_channel();
        Self {
            config,
            keys,
            turn: hold_turn(RuntimeState::default()),
            gemini,
            old: Some(old),
            expected_old_messages: 0,
            prefix: false,
            speech: None,
            output,
            _frames: frames,
            board: Board::new(),
            media: CandidateMedia::new(),
            room,
            recorder,
            loops: RoomLoop {
                pending_round_notice: None,
                deadline_at: tokio::time::Instant::now() + Duration::from_secs(3600),
                recovery_terminal: Some(probe),
                recovery_inputs: Some(inputs),
                recovery_phase: None,
                recovery_reader: None,
                recovery_writes: Default::default(),
                recovery_after: Default::default(),
                restarts: 0,
                deferred_restart: Default::default(),
                resume_recovery: Default::default(),
                interim_review: Default::default(),
                presence: Default::default(),
            },
            events: tokio::sync::Mutex::new(events),
            tx,
            _room_sender: room_sender,
            terminal,
            endpoint,
        }
    }

    async fn run(&mut self) -> ControlFlow<()> {
        let boot = crate::runtime::bootstrap(&self.config, "recovery-test", Some("two-sum"), 45);
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
        let prefix = self.prefix;
        let speech = self.speech.take();
        tokio::time::timeout(Duration::from_secs(10), async {
            let mut context = self.turn.context(
                &mut self.output,
                &mut self.gemini,
                &mut self.board,
                &mut self.media,
            );
            if let Some(handoff) = speech {
                replace_gemini_session(
                    &self.room,
                    &mut context,
                    interview,
                    &mut self.loops,
                    Trigger::GoAway,
                    Some(handoff),
                )
                .await
            } else if prefix {
                spend_deferred_restart(&self.room, &mut context, &mut self.loops, interview, "test")
                    .await
            } else {
                on_gemini_event(&self.room, &mut context, None, &mut self.loops, interview).await
            }
        })
        .await
        .expect("the real room recovery must be bounded in the test")
        .unwrap()
    }

    async fn finish(mut self) {
        self.gemini.shutdown().await.unwrap();
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(2), self.old.take().unwrap())
                .await
                .unwrap()
                .unwrap()
                .len(),
            self.expected_old_messages
        );
    }
}

fn control(tx: &mpsc::UnboundedSender<Packet>, payload: serde_json::Value) {
    tx.send(Packet {
        payload: serde_json::to_vec(&payload).unwrap(),
        topic: Some(TOPIC_CONTROL.into()),
        sender: Some("candidate".into()),
    })
    .unwrap();
}

async fn listener() -> (tokio::net::TcpListener, String) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("ws://{}/", listener.local_addr().unwrap());
    (listener, endpoint)
}

async fn closed_tcp(mut socket: tokio::net::TcpStream) {
    use tokio::io::AsyncReadExt;
    let mut byte = [0];
    match socket.read(&mut byte).await {
        Ok(0) => {}
        Err(error) if error.kind() == std::io::ErrorKind::ConnectionReset => {}
        other => panic!("cancelled socket still carried bytes: {other:?}"),
    }
}

#[tokio::test]
async fn an_end_control_cancels_a_pending_connect_without_releasing_the_peer() {
    if isolated("an_end_control_cancels_a_pending_connect_without_releasing_the_peer") {
        return;
    }
    let (listener, endpoint) = listener().await;
    let mut rig = Rig::new(endpoint).await;
    let tx = rig.tx.clone();
    let (ready, entered) = oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        use tokio::io::AsyncReadExt;
        let mut request = Vec::new();
        while !request.ends_with(b"\r\n\r\n") {
            assert!(request.len() < 4096);
            request.push(socket.read_u8().await.unwrap());
        }
        ready.send(()).unwrap();
        closed_tcp(socket).await;
    });
    let stimulus = async {
        entered.await.unwrap();
        control(
            &tx,
            serde_json::json!({"type":"end_interview", "reason":"user"}),
        );
    };
    let (flow, ()) = tokio::join!(rig.run(), stimulus);
    assert!(flow.is_break());
    assert!(matches!(
        rig.terminal.try_recv().unwrap(),
        Terminal::End { .. }
    ));
    assert_eq!(rig.loops.restarts, 1);
    assert_eq!(rig.turn.activity.live_socket, 1);
    assert_eq!(rig.turn.state.evidence_ledger.metrics.live_setup_count, 0);
    server.await.unwrap();
    rig.finish().await;
}

include!("recovery_parts/entry_cancellation_and_driver_decisions.rs");
include!("recovery_parts/terminal_matrices.rs");
include!("recovery_parts/retry_and_room_waits.rs");
include!("recovery_parts/write_sites_and_replacement_classification.rs");
include!("recovery_parts/tool_work_and_final_sends.rs");
include!("recovery_parts/episode_logs_and_queued_frames.rs");
include!("recovery_parts/ingress_stamping_and_wire_order.rs");

// Declared before running. A prompt is owed with its latency trace open, and a
// tool call is consumed on the live socket before the rotation. The call is
// output, so the prompt itself is answered: the hand-over debt reads
// `prompt=none tool=true generating=true` and the owed text is not replayed.
// The reply is still owed through the continuation, so the replacement keeps
// the trace: `obligation` is the open trace's number (1) and its origin stays
// `prompt`, not `none`/`provider_event`, and the replacement asks once.
#[tokio::test(flavor = "current_thread")]
async fn a_tool_call_consumed_while_a_prompt_is_owed_keeps_the_original_obligation() {
    let name = "a_tool_call_consumed_while_a_prompt_is_owed_keeps_the_original_obligation";
    if isolated_checked(name, |output| {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let start = stderr
            .lines()
            .find(|line| line.starts_with("replacement start: "))
            .map(line_fields)
            .expect("start line");
        assert_eq!(start["obligation"], "1", "{stderr}");
        assert_eq!(start["obligation_origin"], "prompt", "{stderr}");
        let installed = stderr
            .lines()
            .find(|line| line.starts_with("replacement: "))
            .expect("replacement line");
        assert!(
            installed.contains("owed=true (candidate=false prompt=none tool=true generating=true)"),
            "{installed}"
        );
        assert!(installed.contains("obligation=1"), "{installed}");
    }) {
        return;
    }
    let checkpoint = serde_json::json!({"resumable": true, "newHandle": "tool-owed-handle"});
    let (fresh, endpoint) = listener().await;
    let mut rig =
        Rig::with_old_resumption("ws://127.0.0.1:9/".into(), Some(11), Some(checkpoint)).await;
    rig.endpoint = endpoint;
    let old_reader = rig.gemini.inspect_reader_for_test().unwrap();
    wait_for_received_usage(&old_reader).await;
    rig.turn.activity.latency.anchor(Instant::now());
    rig.receive_trail_step(TrailStep::Prompt).await;
    assert!(rig.turn.activity.owes_prompt());
    rig.receive_tool_call("call-1").await;
    assert!(!rig.turn.activity.owes_prompt(), "the call answered it");
    assert!(rig.turn.activity.socket.tool_response_outstanding);
    let server = tokio::spawn(recording_fresh_gemini(fresh, vec![FreshStep::Ready]));
    assert!(rig.run().await.is_continue());
    let _ = rig.old_frames().await;
    let seen = server.await.unwrap();
    assert_eq!(seen.len(), 1);
    let turns = briefing_turns(&seen[0].1);
    assert_eq!(turns.iter().filter(|asked| **asked).count(), 1, "{turns:?}");
}

// The credentialed real-room fixture and its offline pieces.
#[path = "recovery_live.rs"]
mod live;

// Microphone dequeue gap accounting in the room loop.
//
// Declared before running: the recovery driver's audio arm counts the frames it
// takes from the SDK queue, so a loop that was away for 300 ms between two
// frames measures about 300 ms; a stream that ended, and a stream attached
// later, are not gaps however long they were apart; a buffered frame replayed
// from the ingress is not a dequeue.

#[tokio::test]
async fn the_recovery_drivers_audio_arm_measures_the_gap_between_dequeues() {
    if isolated("the_recovery_drivers_audio_arm_measures_the_gap_between_dequeues") {
        return;
    }
    use ::livekit::webrtc::audio_source::AudioSourceOptions;
    use ::livekit::webrtc::audio_source::native::NativeAudioSource;
    let mut rig = Rig::new("ws://127.0.0.1:9/".into()).await;
    let source = NativeAudioSource::new(AudioSourceOptions::default(), 16_000, 1, 0);
    let track = ::livekit::prelude::LocalAudioTrack::create_audio_track(
        "driver-gap",
        ::livekit::prelude::RtcAudioSource::Native(source.clone()),
    );
    rig.media.audio = Some(crate::livekit::media::candidate_audio_stream(
        track.rtc_track(),
    ));
    let boot = crate::runtime::bootstrap(&rig.config, "recovery-test", Some("two-sum"), 45);
    for round in 0..2 {
        source.capture_frame(&numbered_frame(7, 160)).await.unwrap();
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
        driver
            .wait(tokio::time::sleep(Duration::from_millis(40)))
            .await
            .unwrap();
        assert_eq!(driver.ingress.frames.len(), 1, "round {round}");
        if round == 0 {
            // Only one frame so far: nothing is measured.
            assert!(
                driver
                    .context
                    .activity
                    .latency
                    .mic_gaps()
                    .fields()
                    .contains("mic_gap_max_ms=none"),
            );
            drop(driver);
            tokio::time::sleep(Duration::from_millis(300)).await;
        } else {
            let fields = driver.context.activity.latency.mic_gaps().fields();
            let max: u64 = fields
                .split_whitespace()
                .find_map(|t| t.strip_prefix("mic_gap_max_ms="))
                .unwrap()
                .parse()
                .unwrap();
            assert!((300..1000).contains(&max), "{fields}");
            assert!(fields.contains("mic_gap_over_1s=0"), "{fields}");
        }
    }
    rig.finish().await;
}

#[test]
fn a_stream_end_or_a_new_stream_breaks_the_gap_and_replayed_frames_are_not_dequeues() {
    let mut media = crate::livekit::media::CandidateMedia::new();
    let mut latency = crate::livekit::latency::ReplyLatency::default();
    let start = Instant::now();
    let at = |ms: u64| start + Duration::from_millis(ms);
    media.note_mic_dequeue(&mut latency, true, at(0));
    media.note_mic_dequeue(&mut latency, true, at(20));
    let measured = latency.mic_gaps().fields();
    assert!(measured.contains("mic_gap_max_ms=20 "), "{measured}");
    // The stream ended; a minute later a new one began and delivered.
    media.note_mic_dequeue(&mut latency, false, at(30));
    media.note_mic_dequeue(&mut latency, true, at(60_000));
    assert_eq!(
        latency.mic_gaps().fields(),
        measured,
        "no gap across the end"
    );
    // A new stream attached with no end seen in between.
    media.note_mic_dequeue(&mut latency, true, at(60_010));
    media.begin_mic_stream();
    media.note_mic_dequeue(&mut latency, true, at(120_000));
    media.note_mic_dequeue(&mut latency, true, at(120_010));
    let after = latency.mic_gaps().fields();
    assert!(after.contains("mic_gap_max_ms=20 "), "{after}");
    assert!(after.contains("mic_gap_over_1s=0 "), "{after}");
}
