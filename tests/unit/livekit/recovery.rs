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
                        messages.push(serde_json::from_str(frame.to_text().unwrap()).unwrap())
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
        let gemini = crate::gemini::live_session_with_keys_at(&old_endpoint, &keys, &boot, None)
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

#[tokio::test]
async fn an_end_control_cancels_a_pending_setup_without_setup_complete() {
    if isolated("an_end_control_cancels_a_pending_setup_without_setup_complete") {
        return;
    }
    let (listener, endpoint) = listener().await;
    let mut rig = Rig::new(endpoint).await;
    let tx = rig.tx.clone();
    let (ready, entered) = oneshot::channel();
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
        assert!(socket.next().await.unwrap().unwrap().is_text());
        ready.send(()).unwrap();
        closed_tcp(socket.into_inner()).await;
    });
    let stimulus = async {
        entered.await.unwrap();
        control(&tx, serde_json::json!({"type":"end_interview"}));
    };
    let (flow, ()) = tokio::join!(rig.run(), stimulus);
    assert!(flow.is_break());
    assert!(matches!(
        rig.terminal.try_recv().unwrap(),
        Terminal::End { .. }
    ));
    assert_eq!(rig.turn.state.evidence_ledger.metrics.live_setup_count, 0);
    server.await.unwrap();
    rig.finish().await;
}

#[tokio::test]
async fn an_end_control_cancels_the_initial_notice_before_it_is_released() {
    if isolated("an_end_control_cancels_the_initial_notice_before_it_is_released") {
        return;
    }
    let mut rig = Rig::new("ws://127.0.0.1:9/".into()).await;
    let (entered, _release) = rig.recorder.hold_data("interviewer_state", Some(true));
    let tx = rig.tx.clone();
    let stimulus = async {
        entered.notified().await;
        control(&tx, serde_json::json!({"type":"end_interview"}));
    };
    let (flow, ()) = tokio::join!(rig.run(), stimulus);
    assert!(flow.is_break());
    assert_eq!(rig.turn.activity.live_socket, 1);
    assert!(matches!(
        rig.terminal.try_recv().unwrap(),
        Terminal::End { .. }
    ));
    assert!(!rig.recorder.take().iter().any(|record| matches!(record,
        room_link::Sent::Data { message, .. } if message["type"] == "interviewer_state")));
    rig.finish().await;
}

#[tokio::test]
async fn an_end_after_setup_drains_and_discards_the_uncommitted_socket() {
    if isolated("an_end_after_setup_drains_and_discards_the_uncommitted_socket") {
        return;
    }
    let (listener, endpoint) = listener().await;
    let mut rig = Rig::new(endpoint).await;
    let (entered, _release) = rig.recorder.hold_data("interviewer_state", Some(false));
    let tx = rig.tx.clone();
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
        socket.next().await.unwrap().unwrap();
        socket
            .send(Message::Text(r#"{"setupComplete":{}}"#.into()))
            .await
            .unwrap();
        closed_tcp(socket.into_inner()).await;
    });
    let stimulus = async {
        entered.notified().await;
        control(&tx, serde_json::json!({"type":"end_interview"}));
    };
    let (flow, ()) = tokio::join!(rig.run(), stimulus);
    assert!(flow.is_break());
    assert_eq!(
        rig.turn.activity.live_socket, 2,
        "successful setup is still an opened socket"
    );
    assert_eq!(rig.turn.state.evidence_ledger.metrics.live_setup_count, 1);
    assert!(matches!(
        rig.terminal.try_recv().unwrap(),
        Terminal::End { .. }
    ));
    assert!(rig.gemini.send_text("must be closed").await.is_err());
    server.await.unwrap();
    rig.finish().await;
}

#[test]
fn ingress_has_independent_frame_byte_and_age_bounds() {
    let now = Instant::now();
    let frame = |samples| AudioFrame {
        data: std::borrow::Cow::Owned(vec![1; samples]),
        sample_rate: 16000,
        num_channels: 1,
        samples_per_channel: samples as u32,
    };
    let mut ring = Ingress::default();
    for _ in 0..4001 {
        ring.push(frame(1), now);
    }
    assert_eq!(ring.frames.len(), 4000);
    assert_eq!(ring.dropped_bytes, 2);
    assert!(ring.pop(now + INGRESS_AGE).is_none());
    assert_eq!(ring.bytes, 0);
    assert_eq!(ring.dropped_bytes, 8002);
    ring.push(frame(INGRESS_BYTES / 2 + 1), now);
    assert!(ring.is_empty());
    ring.push(frame(INGRESS_BYTES / 2), now);
    ring.push(frame(160), now);
    assert_eq!(ring.frames.len(), 1);
    assert_eq!(ring.bytes, 320);
    assert!(
        matches!(ring.pop(now), Some(CandidateInput::BufferedFrame(frame)) if frame.data.len() == 160)
    );
    ring.push(frame(0), now);
    assert!(ring.is_empty());
}

#[tokio::test]
async fn the_original_deadline_cancels_setup_without_extending_the_interview() {
    if isolated("the_original_deadline_cancels_setup_without_extending_the_interview") {
        return;
    }
    let (listener, endpoint) = listener().await;
    let mut rig = Rig::new(endpoint).await;
    rig.loops.deadline_at = tokio::time::Instant::now() + Duration::from_secs(5);
    let deadline = rig.loops.deadline_at;
    let (ready, entered) = oneshot::channel();
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
        socket.next().await.unwrap().unwrap();
        ready.send(()).unwrap();
        closed_tcp(socket.into_inner()).await;
    });
    let stimulus = async {
        entered.await.unwrap();
        tokio::time::pause();
        tokio::time::advance(Duration::from_secs(5)).await;
        tokio::time::resume();
    };
    let (flow, ()) = tokio::join!(rig.run(), stimulus);
    assert!(flow.is_break());
    assert!(matches!(
        rig.terminal.try_recv().unwrap(),
        Terminal::Deadline
    ));
    assert_eq!(rig.loops.deadline_at, deadline);
    assert_eq!(rig.loops.restarts, 1);
    server.await.unwrap();
    rig.finish().await;
}

#[tokio::test]
async fn an_end_control_cancels_retry_backoff_before_the_next_open() {
    if isolated("an_end_control_cancels_retry_backoff_before_the_next_open") {
        return;
    }
    let (listener, endpoint) = listener().await;
    let mut rig = Rig::new(endpoint).await;
    let recorder = rig.recorder.clone();
    let tx = rig.tx.clone();
    let (probe, mut observed) = mpsc::unbounded_channel();
    rig.loops.recovery_terminal = Some(probe);
    let (ready, rejected) = oneshot::channel();
    let server = tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        while !request.ends_with(b"\r\n\r\n") {
            request.push(socket.read_u8().await.unwrap());
        }
        let (retry_entered, retry_release) = recorder.hold_data("interviewer_state", Some(true));
        retry_release.notify_one();
        socket
            .write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\n\r\n")
            .await
            .unwrap();
        ready.send(retry_entered).unwrap();
        // Keep the listener available; an unintended second open is observable.
        listener
    });
    let stimulus = async {
        let retry_entered = rejected.await.unwrap();
        retry_entered.notified().await;
        end_without_advancing_the_wait(&tx, &mut observed).await;
    };
    let (flow, ()) = tokio::join!(rig.run(), stimulus);
    assert!(flow.is_break());
    assert_eq!(
        rig.loops.restarts, 2,
        "the retry charge survives cancelled backoff"
    );

    let listener = server.await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(50), listener.accept())
            .await
            .is_err()
    );
    rig.finish().await;
}

#[tokio::test]
async fn pending_setup_keeps_thinking_and_latest_editor_before_briefing() {
    if isolated("pending_setup_keeps_thinking_and_latest_editor_before_briefing") {
        return;
    }
    let (listener, endpoint) = listener().await;
    let mut rig = Rig::new(endpoint).await;
    rig.turn.activity.owe_prompt(
        Instant::now(),
        Some("[SYSTEM EVENT] Explain the current editor.".into()),
    );
    let tx = rig.tx.clone();
    let (setup, entered) = oneshot::channel();
    let (release, released) = oneshot::channel();
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
        socket.next().await.unwrap().unwrap();
        setup.send(()).unwrap();
        released.await.unwrap();
        socket
            .send(Message::Text(r#"{"setupComplete":{}}"#.into()))
            .await
            .unwrap();
        let body = socket.next().await.unwrap().unwrap();
        let body: serde_json::Value = serde_json::from_str(body.to_text().unwrap()).unwrap();
        assert_eq!(body["clientContent"]["turnComplete"], false);
        assert!(body.to_string().contains("latest_editor_marker"));
        assert!(socket.next().await.unwrap().unwrap().is_close());
    });
    let stimulus = async {
        entered.await.unwrap();
        control(&tx, serde_json::json!({"type":"thinking", "thinking":true}));
        tx.send(Packet {
            sender: Some("candidate".into()),
            topic: Some(crate::runtime::TOPIC_CODE_UPDATE.into()),
            payload: serde_json::to_vec(
                &serde_json::json!({"code":"fn latest_editor_marker() {}", "language":"rust"}),
            )
            .unwrap(),
        })
        .unwrap();

        // The new setup is allowed only after those queued controls; the
        // driver's biased receive path must reconcile them before the result.
        release.send(()).unwrap();
    };
    let (flow, ()) = tokio::join!(rig.run(), stimulus);
    assert!(flow.is_continue());
    assert!(rig.turn.state.interviewer_silenced());
    assert!(rig.turn.state.code.contains("latest_editor_marker"));
    assert_eq!(rig.turn.state.code_shown, rig.turn.state.code);
    assert!(rig.turn.state.owed_reply_on_resume.is_some());
    assert!(rig.terminal.try_recv().is_err());
    rig.finish().await;
    server.await.unwrap();
}

#[tokio::test]
async fn cancelling_prefix_collection_keeps_received_transcript_and_usage() {
    if isolated("cancelling_prefix_collection_keeps_received_transcript_and_usage") {
        return;
    }
    let mut rig = Rig::new("ws://127.0.0.1:9/".into()).await;
    rig.gemini.shutdown().await.unwrap();
    assert!(rig.old.take().unwrap().await.unwrap().is_empty());
    let (listener, endpoint) = listener().await;
    let (ready, entered) = oneshot::channel();
    let server = tokio::spawn(async move {
        use tokio_tungstenite::tungstenite::{Error as WsError, error::ProtocolError};
        let (socket, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
        socket.next().await.unwrap().unwrap();
        socket
            .send(Message::Text(r#"{"setupComplete":{}}"#.into()))
            .await
            .unwrap();
        let end = socket.next().await.unwrap().unwrap();
        let end: serde_json::Value = serde_json::from_str(end.to_text().unwrap()).unwrap();
        assert_eq!(end["realtimeInput"]["audioStreamEnd"], true);
        socket.send(Message::Text(r#"{"serverContent":{"inputTranscription":{"text":"Keep distinct indices in the answer."}},"usageMetadata":{"promptTokenCount":42}}"#.into())).await.unwrap();
        let note = socket.next().await.unwrap().unwrap();
        let note: serde_json::Value = serde_json::from_str(note.to_text().unwrap()).unwrap();
        assert_eq!(note["clientContent"]["turnComplete"], false);
        ready.send(()).unwrap();
        match socket.next().await {
            None | Some(Err(WsError::Protocol(ProtocolError::ResetWithoutClosingHandshake))) => {}
            Some(Err(WsError::Io(error)))
                if error.kind() == std::io::ErrorKind::ConnectionReset => {}
            other => panic!("cancelled prefix must release its old transport: {other:?}"),
        }
        vec![end, note]
    });
    let boot = crate::runtime::bootstrap(&rig.config, "recovery-test", Some("two-sum"), 45);
    rig.gemini = crate::gemini::live_session_with_keys_at(&endpoint, &rig.keys, &boot, None)
        .await
        .unwrap();
    rig.old = Some(server);
    rig.expected_old_messages = 2;
    rig.prefix = true;
    rig.media.audio_bytes = vec![1, 2];
    rig.media.audio_unconfirmed = true;
    rig.media.identity = Some("candidate".into());
    rig.loops.deferred_restart = DeferredRestart(true);
    rig.turn
        .activity
        .note_candidate_voice_activity(true, Instant::now());
    let tx = rig.tx.clone();
    let stimulus = async {
        entered.await.unwrap();
        control(&tx, serde_json::json!({"type":"end_interview"}));
    };
    let (flow, ()) = tokio::join!(rig.run(), stimulus);
    assert!(flow.is_break());
    assert!(matches!(
        rig.terminal.try_recv().unwrap(),
        Terminal::End { .. }
    ));
    assert!(
        rig.turn
            .state
            .transcript
            .iter()
            .any(|line| line.contains("Keep distinct indices"))
    );
    assert_eq!(rig.turn.activity.live_usage.samples, 1);
    assert_eq!(rig.turn.activity.live_usage.prompt, 42);
    assert_eq!(rig.loops.restarts, 0, "no replacement open began");
    rig.finish().await;
}

#[tokio::test]
async fn recovery_preserves_never_sent_tail_and_does_not_replay_an_uncertain_write() {
    if isolated("recovery_preserves_never_sent_tail_and_does_not_replay_an_uncertain_write") {
        return;
    }
    use base64::{Engine, engine::general_purpose::STANDARD};
    for uncertain in [false, true] {
        let (listener, endpoint) = listener().await;
        let mut rig = Rig::new(endpoint).await;
        rig.media.audio_bytes = vec![1, 2];
        rig.media.audio_unconfirmed = uncertain;
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
            socket.next().await.unwrap().unwrap();
            socket
                .send(Message::Text(r#"{"setupComplete":{}}"#.into()))
                .await
                .unwrap();
            for _ in 0..2 {
                socket.next().await.unwrap().unwrap();
            }
            let audio = socket.next().await.unwrap().unwrap();
            let audio: serde_json::Value = serde_json::from_str(audio.to_text().unwrap()).unwrap();
            let bytes = STANDARD
                .decode(audio["realtimeInput"]["audio"]["data"].as_str().unwrap())
                .unwrap();
            let mut expected = if uncertain { Vec::new() } else { vec![1, 2] };
            expected.extend([4, 3].repeat(160));
            assert_eq!(bytes, expected);
            assert!(socket.next().await.unwrap().unwrap().is_close());
        });
        assert!(rig.run().await.is_continue());
        assert_eq!(
            rig.media.audio_bytes,
            if uncertain { Vec::new() } else { vec![1, 2] }
        );
        let input = CandidateInput::Frame(AudioFrame {
            data: vec![0x0304; 160].into(),
            sample_rate: 16000,
            num_channels: 1,
            samples_per_channel: 160,
        });
        on_candidate_input(
            &mut rig.turn.context(
                &mut rig.output,
                &mut rig.gemini,
                &mut rig.board,
                &mut rig.media,
            ),
            input,
            1500,
        )
        .await;
        assert!(!rig.media.audio_unconfirmed);
        assert!(rig.media.audio_bytes.is_empty());
        rig.finish().await;
        server.await.unwrap();
    }
}

#[tokio::test]
async fn buffered_audio_keeps_its_order_and_yield_without_refreshing_live_voice() {
    if isolated("buffered_audio_keeps_its_order_and_yield_without_refreshing_live_voice") {
        return;
    }
    use base64::{Engine, engine::general_purpose::STANDARD};
    let (listener, endpoint) = listener().await;
    let mut rig = Rig::new(endpoint).await;
    let arrived = Instant::now() - Duration::from_secs(5);
    rig.media.voiced_at = Some(arrived);
    rig.media.recovery_audio.push(
        AudioFrame {
            data: vec![7; 80].into(),
            sample_rate: 16000,
            num_channels: 1,
            samples_per_channel: 80,
        },
        arrived,
    );
    rig.media.recovery_audio.yielded(arrived);
    rig.media.recovery_audio.push(
        AudioFrame {
            data: vec![9; 160].into(),
            sample_rate: 16000,
            num_channels: 1,
            samples_per_channel: 160,
        },
        arrived,
    );
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
        socket.next().await.unwrap().unwrap();
        socket
            .send(Message::Text(r#"{"setupComplete":{}}"#.into()))
            .await
            .unwrap();
        for _ in 0..2 {
            socket.next().await.unwrap().unwrap();
        }
        let mut values = Vec::new();
        for _ in 0..3 {
            let frame = socket.next().await.unwrap().unwrap();
            values
                .push(serde_json::from_str::<serde_json::Value>(frame.to_text().unwrap()).unwrap());
        }
        assert_eq!(
            STANDARD
                .decode(
                    values[0]["realtimeInput"]["audio"]["data"]
                        .as_str()
                        .unwrap()
                )
                .unwrap(),
            [7, 0].repeat(80)
        );
        assert_eq!(values[1]["realtimeInput"]["audioStreamEnd"], true);
        assert_eq!(
            STANDARD
                .decode(
                    values[2]["realtimeInput"]["audio"]["data"]
                        .as_str()
                        .unwrap()
                )
                .unwrap(),
            [9, 0].repeat(160)
        );
        assert!(socket.next().await.unwrap().unwrap().is_close());
    });
    assert!(rig.run().await.is_continue());
    for _ in 0..3 {
        let input = next_candidate_input(&mut rig.media.audio, &mut rig.media.recovery_audio).await;
        on_candidate_input(
            &mut rig.turn.context(
                &mut rig.output,
                &mut rig.gemini,
                &mut rig.board,
                &mut rig.media,
            ),
            input,
            1500,
        )
        .await;
    }
    assert_eq!(rig.media.voiced_at, Some(arrived));
    assert!(rig.media.recovery_audio.is_empty());
    rig.finish().await;
    server.await.unwrap();
}

async fn end_without_advancing_the_wait(
    tx: &mpsc::UnboundedSender<Packet>,
    observed: &mut mpsc::UnboundedReceiver<Terminal>,
) {
    tokio::time::pause();
    let frozen = tokio::time::Instant::now();
    control(tx, serde_json::json!({"type":"end_interview"}));
    let mut terminal = None;
    for _ in 0..1024 {
        if let Ok(value) = observed.try_recv() {
            terminal = Some(value);
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(
        tokio::time::Instant::now(),
        frozen,
        "the operation timeout must remain unspent"
    );
    tokio::time::resume();
    assert!(
        matches!(terminal, Some(Terminal::End { .. })),
        "end was not handled while the operation was pending"
    );
}

#[tokio::test]
async fn end_cancels_a_close_while_its_writer_and_timeout_remain_blocked() {
    if isolated("end_cancels_a_close_while_its_writer_and_timeout_remain_blocked") {
        return;
    }
    let mut rig = Rig::new("ws://127.0.0.1:9/".into()).await;
    rig.gemini.shutdown().await.unwrap();
    rig.old.take().unwrap().await.unwrap();
    let listener = tokio::net::TcpSocket::new_v4().unwrap();
    listener.set_recv_buffer_size(1024).unwrap();
    listener.bind("127.0.0.1:0".parse().unwrap()).unwrap();
    let listener = listener.listen(1).unwrap();
    let endpoint = format!("ws://{}/", listener.local_addr().unwrap());
    let (read, reading) = oneshot::channel();
    let server = tokio::spawn(async move {
        use tokio::io::AsyncReadExt;
        let (socket, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
        socket.next().await.unwrap().unwrap();
        socket
            .send(Message::Text(r#"{"setupComplete":{}}"#.into()))
            .await
            .unwrap();
        let mut socket = socket.into_inner();
        reading.await.unwrap();
        let mut bytes = [0; 65536];
        loop {
            match socket.read(&mut bytes).await {
                Ok(0) => break,
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::ConnectionReset => break,
                other => panic!("closing the retired TCP stream failed: {other:?}"),
            }
        }
        Vec::new()
    });
    let boot = crate::runtime::bootstrap(&rig.config, "recovery-test", Some("two-sum"), 45);
    rig.gemini = crate::gemini::live_session_with_keys_at(&endpoint, &rig.keys, &boot, None)
        .await
        .unwrap();
    rig.old = Some(server);
    let mut blocked = false;
    for _ in 0..64 {
        if tokio::time::timeout(
            Duration::from_millis(30),
            rig.gemini
                .writer_for_test()
                .send(Message::Binary(vec![0; 256 * 1024].into())),
        )
        .await
        .is_err()
        {
            blocked = true;
            break;
        }
    }
    assert!(
        blocked,
        "the fixture must establish real writer backpressure"
    );
    let (phase, mut phases) = mpsc::unbounded_channel();
    rig.loops.recovery_phase = Some(phase);
    let (probe, mut observed) = mpsc::unbounded_channel();
    rig.loops.recovery_terminal = Some(probe);
    let tx = rig.tx.clone();
    let stimulus = async {
        while phases.recv().await != Some("close") {}
        end_without_advancing_the_wait(&tx, &mut observed).await;
        read.send(()).unwrap();
    };
    let (flow, ()) = tokio::join!(rig.run(), stimulus);
    assert!(flow.is_break());
    assert_eq!(rig.turn.activity.live_socket, 1);
    rig.finish().await;
}

#[test]
fn trimming_audio_carries_the_boundary_before_retained_later_frames() {
    let at = Instant::now();
    let frame = |value| AudioFrame {
        data: vec![value; 160].into(),
        sample_rate: 16000,
        num_channels: 1,
        samples_per_channel: 160,
    };
    let mut ring = Ingress::default();
    ring.push(frame(7), at);
    ring.yielded(at);
    ring.push(frame(9), at + Duration::from_secs(5));
    assert!(matches!(
        ring.pop(at),
        Some(CandidateInput::BufferedFrame(_))
    ));
    assert!(matches!(
        ring.pop(at + INGRESS_AGE),
        Some(CandidateInput::Yield)
    ));
    assert!(
        matches!(ring.pop(at + INGRESS_AGE), Some(CandidateInput::BufferedFrame(frame)) if frame.data[0] == 9)
    );
    assert!(ring.is_empty());
}

#[tokio::test]
async fn a_release_during_a_notice_requests_a_reply_when_only_markers_are_buffered() {
    if isolated("a_release_during_a_notice_requests_a_reply_when_only_markers_are_buffered") {
        return;
    }
    let mut rig = Rig::new("ws://127.0.0.1:9/".into()).await;
    rig.turn.state.thinking_hold = crate::agent::ThinkingHold::Held {
        since: Instant::now(),
    };
    rig.turn.state.owed_reply_on_resume =
        Some(Some("[SYSTEM EVENT] Answer the original question.".into()));
    rig.turn.state.announce_thinking();
    rig.turn.state.earlier_steps_pending.push("design");
    let old = rig.gemini.retire();
    let (mut fresh, server) = fake_recording_socket(None, 2).await;
    let (entered, release) = rig.recorder.hold_data("thinking_state", None);
    let tx = rig.tx.clone();
    let boot = crate::runtime::bootstrap(&rig.config, "recovery-test", Some("two-sum"), 45);
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
    {
        let mut context = rig.turn.context(
            &mut rig.output,
            &mut rig.gemini,
            &mut rig.board,
            &mut rig.media,
        );
        let mut driver = Driver::new(&rig.room, &mut context, &mut rig.loops, interview);
        let stimulus = async {
            entered.notified().await;
            control(
                &tx,
                serde_json::json!({"type":"thinking", "thinking":false}),
            );
            release.notify_one();
        };
        let (replied, ()) = tokio::join!(driver.drain(&mut fresh), stimulus);
        assert!(replied.unwrap());
        assert!(!driver.context.state.interviewer_silenced());
        assert!(!driver.ingress.has_audio());
        assert!(driver.context.activity.owes_prompt());
        assert!(driver.context.state.owed_reply_on_resume.is_none());
        assert!(!driver.pending());
    }
    let messages = server.await.unwrap();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0]["clientContent"]["turnComplete"], false);
    assert_eq!(messages[1]["clientContent"]["turnComplete"], true);
    let body = messages[1].to_string();
    assert!(body.contains("original question"));
    assert_eq!(
        body.matches("Earlier step references carried with this request")
            .count(),
        1
    );
    fresh.shutdown().await.unwrap();
    rig.gemini = old;
    rig.finish().await;
}

#[tokio::test]
async fn reference_debt_survives_noop_packets_and_action_supersession() {
    if isolated("reference_debt_survives_noop_packets_and_action_supersession") {
        return;
    }
    let mut rig = Rig::new("ws://127.0.0.1:9/".into()).await;
    let old = rig.gemini.retire();
    let boot = crate::runtime::bootstrap(&rig.config, "recovery-test", Some("two-sum"), 45);
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
    {
        let mut context = rig.turn.context(
            &mut rig.output,
            &mut rig.gemini,
            &mut rig.board,
            &mut rig.media,
        );
        let mut driver = Driver::new(&rig.room, &mut context, &mut rig.loops, interview);
        driver.context.state.earlier_steps_pending.push("design");
        let noop = serde_json::to_vec(&serde_json::json!({"type":"unknown"})).unwrap();
        driver.packet(Some(TOPIC_CONTROL), Some("candidate"), &noop);
        assert_eq!(driver.context.state.earlier_steps_pending, ["design"]);
        for sender in [None, Some("bystander")] {
            driver.packet(Some(TOPIC_CONTROL), sender, br#"{"type":"end_interview"}"#);
            assert!(driver.terminal.is_none());
        }
        driver.context.state.owed_reply_on_resume = Some(Some("raw-platform-request".into()));
        for step in ["test", "analyze"] {
            driver.packet(
                Some(TOPIC_CONTROL),
                Some("candidate"),
                br#"{"type":"thinking","thinking":true}"#,
            );
            driver.context.state.earlier_steps_pending.push(step);
            driver.packet(
                Some(TOPIC_CONTROL),
                Some("candidate"),
                br#"{"type":"thinking","thinking":false}"#,
            );
        }
        let action = driver.action.as_ref().unwrap();
        for step in ["design", "test", "analyze"] {
            assert!(action.earlier.contains(&step));
            assert!(action.text.contains(step));
        }
        assert_eq!(
            driver.context.state.owed_reply_on_resume,
            Some(Some("raw-platform-request".into()))
        );
        assert!(driver.context.state.earlier_steps_named.is_empty());
        let mut closed = driver.context.gemini.retire();
        let (ok, spoke) = driver.flush_actions(&mut closed).await.unwrap();
        assert!(!ok && !spoke);
        for step in ["design", "test", "analyze"] {
            assert!(driver.context.state.earlier_steps_pending.contains(&step));
        }
        assert!(driver.context.state.earlier_steps_named.is_empty());
        driver.held_contexts.push_back("unsent held summary".into());
        driver.held_bytes = "unsent held summary".len();
        driver.round = Some("skipped");
        driver.loops.pending_round_notice = Some("started");
        assert!(!driver.drain(&mut closed).await.unwrap());
        assert!(
            driver.loops.pending_round_notice.is_none(),
            "the newer successful notice supersedes the older failed snapshot"
        );
        let rounds = rig
            .recorder
            .take()
            .into_iter()
            .filter_map(|sent| match sent {
                room_link::Sent::Data { message, .. } if message["type"] == "round_state" => {
                    Some(message)
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            rounds.len(),
            1,
            "failed model context must not skip the room notice"
        );
        assert_eq!(rounds[0]["status"], "skipped");
        driver.round = Some("started");
        driver.loops.pending_round_notice = Some("skipped");
        driver.evictions.push_back("duplicate-agent-fixture".into());
        driver.preserve_room_work();
        assert_eq!(driver.loops.pending_round_notice, Some("started"));
        assert!(driver.round.is_none());
        assert!(driver.evictions.is_empty());
        assert_eq!(driver.evictions_omitted, 1);
    }
    rig.gemini = old;
    rig.finish().await;
}

#[test]
fn expired_audio_cannot_defer_a_reply_and_empty_boundaries_are_coalesced() {
    let now = Instant::now();
    let mut ring = Ingress::default();
    ring.yielded(now - INGRESS_AGE);
    ring.push(
        AudioFrame {
            data: vec![7; 160].into(),
            sample_rate: 16000,
            num_channels: 1,
            samples_per_channel: 160,
        },
        now - INGRESS_AGE,
    );
    assert!(!ring.has_audio(), "expired bytes cannot defer the action");
    ring.yielded(now);
    assert!(matches!(ring.pop(now), Some(CandidateInput::Yield)));
    assert!(ring.pop(now).is_none(), "one empty boundary is sufficient");
    assert_eq!(ring.dropped_bytes, 320);
}
#[tokio::test]
async fn a_test_summary_received_during_a_notice_is_drained_before_commit() {
    if isolated("a_test_summary_received_during_a_notice_is_drained_before_commit") {
        return;
    }
    let mut rig = Rig::new("ws://127.0.0.1:9/".into()).await;
    rig.turn.state.thinking_hold = crate::agent::ThinkingHold::Held {
        since: Instant::now(),
    };
    rig.turn.state.code = "def solve(): return 7".into();
    rig.turn.state.language = "python".into();
    rig.turn.activity.last_test_reaction = Instant::now() - Duration::from_secs(100);
    rig.turn.state.announce_thinking();
    let old = rig.gemini.retire();
    let (mut fresh, server) = fake_recording_socket(None, 1).await;
    let (entered, release) = rig.recorder.hold_data("thinking_state", None);
    let tx = rig.tx.clone();
    let boot = crate::runtime::bootstrap(&rig.config, "recovery-test", Some("two-sum"), 45);
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
    {
        let mut context = rig.turn.context(
            &mut rig.output,
            &mut rig.gemini,
            &mut rig.board,
            &mut rig.media,
        );
        let mut driver = Driver::new(&rig.room, &mut context, &mut rig.loops, interview);
        let stimulus = async {
            entered.notified().await;
            tx.send(Packet {
                topic: Some(crate::runtime::TOPIC_TEST_RESULTS.into()),
                sender: Some("candidate".into()),
                payload: serde_json::to_vec(&serde_json::json!({
                    "passed": 0, "total": 1, "code":"def solve(): return 7",
                    "language":"python"
                }))
                .unwrap(),
            })
            .unwrap();
            release.notify_one();
        };
        let (replied, ()) = tokio::join!(driver.drain(&mut fresh), stimulus);
        assert!(!replied.unwrap());
        assert!(driver.context.state.interviewer_silenced());
        assert!(!driver.ingress.has_audio());
        assert_eq!(driver.context.state.test_runs, 1);
        assert!(!driver.pending());
    }
    let messages = server.await.unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["clientContent"]["turnComplete"], false);
    let body = messages[0].to_string();
    assert!(
        body.contains("0/1") || body.contains("0 of 1"),
        "the late test summary must reach the wire: {body}"
    );
    fresh.shutdown().await.unwrap();
    rig.gemini = old;
    rig.finish().await;
}

#[tokio::test]
async fn the_last_correction_keeps_its_reply_and_preserves_newer_editor_debt() {
    if isolated("the_last_correction_keeps_its_reply_and_preserves_newer_editor_debt") {
        return;
    }
    let (listener, endpoint) = listener().await;
    let mut rig = Rig::new(endpoint).await;
    rig.turn.state.example_drawing_enabled = true;
    rig.turn.state.thinking_hold = crate::agent::ThinkingHold::Held {
        since: Instant::now(),
    };
    rig.turn.state.owed_reply_on_resume = Some(Some("raw-question-marker".into()));
    rig.turn
        .activity
        .owe_prompt(Instant::now(), Some("raw-question-marker".into()));
    let mut signals = Vec::new();
    for _ in 0..2 {
        let entered = std::sync::Arc::new(tokio::sync::Notify::new());
        let release = std::sync::Arc::new(tokio::sync::Notify::new());
        rig.loops.recovery_writes.push_back(WriteBarrier {
            phase: "recovery_brief",
            entered: entered.clone(),
            release: release.clone(),
        });
        signals.push((entered, release));
    }
    let tx = rig.tx.clone();
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
        socket.next().await.unwrap().unwrap();
        socket
            .send(Message::Text(r#"{"setupComplete":{}}"#.into()))
            .await
            .unwrap();
        let mut messages = Vec::new();
        loop {
            let frame = socket.next().await.unwrap().unwrap();
            if frame.is_close() {
                break;
            }
            messages
                .push(serde_json::from_str::<serde_json::Value>(frame.to_text().unwrap()).unwrap());
        }
        messages
    });
    let stimulus = async {
        signals[0].0.notified().await;
        control(
            &tx,
            serde_json::json!({"type":"thinking", "thinking":false}),
        );
        signals[0].1.notify_one();
        signals[1].0.notified().await;
        tx.send(Packet {
            sender: Some("candidate".into()),
            topic: Some(crate::runtime::TOPIC_CODE_UPDATE.into()),
            payload: serde_json::to_vec(
                &serde_json::json!({"code":"fn late_editor_marker() {}", "language":"rust"}),
            )
            .unwrap(),
        })
        .unwrap();
        control(
            &tx,
            serde_json::json!({"type":"yield_turn", "surface":"example"}),
        );
        signals[1].1.notify_one();
    };
    let (flow, ()) = tokio::join!(rig.run(), stimulus);
    assert!(flow.is_continue());
    assert!(
        !rig.turn.activity.socket.discarding_output,
        "the final correction has no following correction to discard for"
    );
    assert!(rig.turn.state.code.contains("late_editor_marker"));
    assert!(
        !rig.turn.state.code_shown.contains("late_editor_marker"),
        "an older prepared briefing cannot mark a newer editor delivered"
    );
    rig.finish().await;
    let messages = server.await.unwrap();
    assert!(
        messages
            .iter()
            .any(|message| message.to_string().contains("paused their Example drawing")),
        "the new request received during the last briefing must also be sent"
    );
    assert!(
        messages.len() >= 4,
        "both prepared briefs must reach the wire"
    );
    assert!(
        messages
            .iter()
            .any(|message| message["clientContent"]["turnComplete"] == true)
    );
}

#[test]
fn republish_after_commit_keeps_retained_whole_frames_before_the_new_stream() {
    for uncertain in [false, true] {
        let now = Instant::now();
        let mut media = CandidateMedia::new();
        media.audio_bytes = vec![1, 2];
        media.audio_unconfirmed = uncertain;
        media.recovery_audio.push(
            AudioFrame {
                data: vec![7; 160].into(),
                sample_rate: 16000,
                num_channels: 1,
                samples_per_channel: 160,
            },
            now,
        );
        media.begin_audio_publication();
        assert_eq!(
            media.audio_bytes,
            if uncertain { vec![] } else { vec![1, 2] }
        );
        assert_eq!(media.quarantined_audio_bytes, if uncertain { 2 } else { 0 });
        assert!(!media.audio_unconfirmed);
        assert!(
            matches!(media.recovery_audio.pop(now), Some(CandidateInput::BufferedFrame(frame)) if frame.data[0] == 7)
        );
        assert!(matches!(
            media.recovery_audio.pop(now),
            Some(CandidateInput::Yield)
        ));
        assert!(media.recovery_audio.pop(now).is_none());
        media.begin_audio_publication();
        assert!(
            media.recovery_audio.is_empty(),
            "startup does not invent an audio-end"
        );
    }
}

#[derive(Clone, Copy, Debug)]
enum PendingOpen {
    Upgrade,
    Setup,
    ReconnectedNotice,
}

#[derive(Clone, Copy, Debug)]
enum MatrixStop {
    Deadline,
    RoomDisconnected,
}

async fn observe_matrix_stop(
    cause: MatrixStop,
    deadline: tokio::time::Instant,
    room: &mpsc::UnboundedSender<RoomEvent>,
    observed: &mut mpsc::UnboundedReceiver<Terminal>,
) {
    // All peer/publication barriers precede pause. Cooperative polling keeps
    // Tokio from auto-advancing an unrelated operation or fixture timeout.
    tokio::time::pause();
    match cause {
        MatrixStop::Deadline => {
            tokio::time::advance(deadline.saturating_duration_since(tokio::time::Instant::now()))
                .await;
        }
        MatrixStop::RoomDisconnected => {
            room.send(RoomEvent::Disconnected {
                reason: DisconnectReason::ClientInitiated,
            })
            .unwrap();
        }
    }
    let frozen = tokio::time::Instant::now();
    let mut terminal = None;
    for _ in 0..4096 {
        if let Ok(value) = observed.try_recv() {
            terminal = Some(value);
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(tokio::time::Instant::now(), frozen);
    tokio::time::resume();
    assert!(
        matches!(
            (cause, terminal),
            (MatrixStop::Deadline, Some(Terminal::Deadline))
                | (MatrixStop::RoomDisconnected, Some(Terminal::RoomClosed))
        ),
        "the original terminal cause must win while the phase remains pending"
    );
}

#[tokio::test]
async fn deadline_and_room_disconnect_cancel_each_pending_open_phase() {
    if isolated("deadline_and_room_disconnect_cancel_each_pending_open_phase") {
        return;
    }
    let mut rows = 0;
    for phase in [
        PendingOpen::Upgrade,
        PendingOpen::Setup,
        PendingOpen::ReconnectedNotice,
    ] {
        for cause in [MatrixStop::Deadline, MatrixStop::RoomDisconnected] {
            let (listener, endpoint) = listener().await;
            let mut rig = Rig::new(endpoint).await;
            rig.loops.deadline_at = tokio::time::Instant::now() + Duration::from_secs(8);
            let deadline = rig.loops.deadline_at;
            let room = rig._room_sender.clone();
            let (probe, mut observed) = mpsc::unbounded_channel();
            rig.loops.recovery_terminal = Some(probe);
            let (notice, _release) = rig.recorder.hold_data("interviewer_state", Some(false));
            let (ready, entered) = oneshot::channel();
            let (finish, finished) = oneshot::channel();
            let instructions =
                crate::runtime::bootstrap(&rig.config, "recovery-test", Some("two-sum"), 45)
                    .instructions;
            let server = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                if matches!(phase, PendingOpen::Upgrade) {
                    use tokio::io::AsyncReadExt;
                    let mut request = Vec::new();
                    while !request.ends_with(b"\r\n\r\n") {
                        assert!(request.len() < 4096);
                        request.push(socket.read_u8().await.unwrap());
                    }
                    ready.send(()).unwrap();
                    closed_tcp(socket).await;
                } else {
                    let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
                    let setup = socket.next().await.unwrap().unwrap();
                    let setup: serde_json::Value =
                        serde_json::from_str(setup.to_text().unwrap()).unwrap();
                    assert_eq!(
                        setup["setup"]["systemInstruction"]["parts"][0]["text"],
                        instructions
                    );
                    if matches!(phase, PendingOpen::ReconnectedNotice) {
                        socket
                            .send(Message::Text(r#"{"setupComplete":{}}"#.into()))
                            .await
                            .unwrap();
                    }
                    ready.send(()).unwrap();
                    closed_tcp(socket.into_inner()).await;
                }

                // Keep the listener through all fixture teardown. A queued
                // extra attempt is observable without a sleep-based negative.
                finished.await.unwrap();
                assert_eq!(
                    listener.into_std().unwrap().accept().unwrap_err().kind(),
                    std::io::ErrorKind::WouldBlock
                );
                1usize
            });
            let stimulus = async {
                entered.await.unwrap();
                if matches!(phase, PendingOpen::ReconnectedNotice) {
                    notice.notified().await;
                }
                observe_matrix_stop(cause, deadline, &room, &mut observed).await;
            };
            let (flow, ()) = tokio::join!(rig.run(), stimulus);
            assert!(flow.is_break(), "phase={phase:?} cause={cause:?}");
            assert_eq!(rig.loops.deadline_at, deadline);
            assert_eq!(rig.loops.restarts, 1);
            let completed_setup = u32::from(matches!(phase, PendingOpen::ReconnectedNotice));
            assert_eq!(
                rig.turn.activity.live_socket,
                1 + u64::from(completed_setup)
            );
            assert_eq!(
                rig.turn.state.evidence_ledger.metrics.live_setup_count,
                completed_setup
            );
            assert!(
                rig.gemini
                    .send_text("must remain a closed placeholder")
                    .await
                    .is_err()
            );
            assert!(observed.try_recv().is_err(), "one terminal intent only");
            assert!(!rig.recorder.take().iter().any(|record| matches!(record,
                room_link::Sent::Data { message, .. } if message["type"] == "interviewer_state" && message["reconnecting"] == false)));
            rig.finish().await;
            finish.send(()).unwrap();
            assert_eq!(server.await.unwrap(), 1);
            rows += 1;
        }
    }
    assert_eq!(rows, 6, "every phase/cause row must execute");
}

#[tokio::test]
async fn absence_and_room_disconnect_really_leave_without_a_report_probe() {
    if isolated("absence_and_room_disconnect_really_leave_without_a_report_probe") {
        return;
    }
    for absent in [true, false] {
        let (listener, endpoint) = listener().await;
        let mut rig = Rig::new(endpoint).await;
        rig.loops.recovery_terminal = None;
        let room = rig._room_sender.clone();
        let (ready, entered) = oneshot::channel();
        let server = if absent {
            rig.loops.presence.left(
                Instant::now()
                    .checked_sub(CANDIDATE_ABSENCE_LIMIT)
                    .expect("the fixture needs a representable absence interval"),
            );
            drop(ready);
            None
        } else {
            Some(tokio::spawn(async move {
                let (socket, _) = listener.accept().await.unwrap();
                let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
                socket.next().await.unwrap().unwrap();
                ready.send(()).unwrap();
                closed_tcp(socket.into_inner()).await;
            }))
        };
        let (completed, mut completion) = oneshot::channel();
        let running = async {
            let result = rig.run().await;
            completed.send(()).unwrap();
            result
        };
        let stimulus = async {
            if !absent {
                entered.await.unwrap();
                tokio::time::pause();
                let frozen = tokio::time::Instant::now();
                room.send(RoomEvent::Disconnected {
                    reason: DisconnectReason::ClientInitiated,
                })
                .unwrap();
                let mut done = false;
                for _ in 0..4096 {
                    if completion.try_recv().is_ok() {
                        done = true;
                        break;
                    }
                    tokio::task::yield_now().await;
                }
                assert_eq!(tokio::time::Instant::now(), frozen);
                tokio::time::resume();
                assert!(done, "real leave dispatch must not wait for setup timeout");
            } else {
                completion.await.unwrap();
            }
        };
        let (flow, ()) = tokio::join!(running, stimulus);
        assert!(flow.is_break());
        assert_eq!(rig.loops.restarts, usize::from(!absent));
        assert_eq!(rig.turn.activity.live_socket, 1);
        let records = rig.recorder.take();
        assert_eq!(
            records
                .iter()
                .filter(|record| matches!(record, room_link::Sent::Left))
                .count(),
            1
        );
        assert!(!records.iter().any(|record| matches!(record,
            room_link::Sent::Data { topic, .. } if topic.as_deref() == Some(crate::runtime::TOPIC_REPORT))));
        rig.finish().await;
        if let Some(server) = server {
            server.await.unwrap();
        }
    }
}

async fn wait_for_received_usage(reader: &crate::gemini::LiveReaderInspection) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while reader.pending_usage_count() == 0 {
            assert!(!reader.is_finished(), "the peer keeps the reader alive");
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the real reader must observe the peer's usage before cancellation");
    assert_eq!(reader.pending_usage_count(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn cancellation_joins_live_readers_and_accounts_old_and_rejected_usage_once() {
    if isolated_checked(
        "cancellation_joins_live_readers_and_accounts_old_and_rejected_usage_once",
        |output| {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let lines = stderr
                .lines()
                .filter(|line| line.starts_with("codetrial live_turn_usage "))
                .collect::<Vec<_>>();
            assert_eq!(
                lines.len(),
                3,
                "each socket observation must be logged once"
            );
            for (line, socket, cause, prompt, event) in [
                (lines[0], 1, "tool", 11, 1),
                (lines[1], 1, "tool", 11, 1),
                (lines[2], 2, "candidate", 29, 2),
            ] {
                for field in [
                    format!("socket={socket}"),
                    format!("cause={cause}"),
                    format!("prompt_tokens={prompt}"),
                    format!("usage_event={event}"),
                ] {
                    assert!(
                        line.split_whitespace().any(|value| value == field),
                        "missing {field} in {line}"
                    );
                }
            }
        },
    ) {
        return;
    }
    for after_setup in [false, true] {
        let (listener, endpoint) = listener().await;
        let mut unopened = Some(listener);
        let mut rig = Rig::with_old_usage(endpoint, Some(11)).await;
        rig.gemini.input_cause = Some("tool");
        let old = rig.gemini.inspect_reader_for_test().unwrap();
        wait_for_received_usage(&old).await;
        assert!(!old.is_finished());
        let (reader, mut readers) = mpsc::unbounded_channel();
        rig.loops.recovery_reader = Some(reader);
        let (terminal, mut observed) = mpsc::unbounded_channel();
        rig.loops.recovery_terminal = Some(terminal);
        let (entered, _release) = rig
            .recorder
            .hold_data("interviewer_state", Some(!after_setup));
        let server = if after_setup {
            let listener = unopened.take().unwrap();
            Some(tokio::spawn(async move {
                let (socket, _) = listener.accept().await.unwrap();
                let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
                socket.next().await.unwrap().unwrap();
                socket
                    .send(Message::Text(r#"{"setupComplete":{}}"#.into()))
                    .await
                    .unwrap();
                socket
                    .send(Message::Text(
                        r#"{"usageMetadata":{"promptTokenCount":29}}"#.into(),
                    ))
                    .await
                    .unwrap();

                // No close response or peer EOF can finish the reader for the
                // cancellation path. Only transport release ends this wait.
                closed_tcp(socket.into_inner()).await;
            }))
        } else {
            None
        };
        let tx = rig.tx.clone();
        let fresh_state = std::cell::RefCell::new(None::<crate::gemini::LiveReaderInspection>);
        let running = async {
            let flow = rig.run().await;

            // Capture ownership state in the very poll returning recovery.
            // Waiting for the other join branch could let Drop's queued abort
            // finish and hide a missing explicit reader join.
            assert!(
                old.is_finished(),
                "old reader must finish before recovery returns"
            );
            assert_eq!(old.pending_usage_count(), 0);
            let fresh = fresh_state.borrow();
            assert_eq!(fresh.is_some(), after_setup);
            if let Some(fresh) = fresh.as_ref() {
                assert!(
                    fresh.is_finished(),
                    "rejected fresh reader must finish before recovery returns"
                );
                assert_eq!(fresh.pending_usage_count(), 0);
            }
            flow
        };
        let stimulus = async {
            entered.notified().await;
            let fresh = if after_setup {
                let fresh = readers.recv().await.unwrap();
                wait_for_received_usage(&fresh).await;
                assert!(!fresh.is_finished());
                Some(fresh)
            } else {
                None
            };
            fresh_state.replace(fresh);

            // Receipt and all real I/O barriers precede this frozen-clock
            // terminal observation. There is no timer allowance for cleanup.
            end_without_advancing_the_wait(&tx, &mut observed).await;
        };
        let (flow, ()) = tokio::join!(running, stimulus);
        assert!(flow.is_break());
        if !after_setup {
            assert!(
                readers.try_recv().is_err(),
                "no new setup before the initial notice"
            );
        }
        assert_eq!(
            rig.turn.activity.live_usage.samples,
            if after_setup { 2 } else { 1 }
        );
        assert_eq!(
            rig.turn.activity.live_usage.prompt,
            if after_setup { 40 } else { 11 }
        );
        assert_eq!(rig.loops.restarts, 1);
        assert_eq!(
            rig.turn.activity.live_socket,
            if after_setup { 2 } else { 1 }
        );
        assert_eq!(
            rig.turn.state.evidence_ledger.metrics.live_setup_count,
            u32::from(after_setup)
        );
        rig.finish().await;
        if let Some(server) = server {
            server.await.unwrap();
        }
        if let Some(listener) = unopened {
            assert_eq!(
                listener.into_std().unwrap().accept().unwrap_err().kind(),
                std::io::ErrorKind::WouldBlock
            );
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum OpenStop {
    End,
    Deadline,
    RoomDisconnected,
    EventsEof,
    Absent,
}

async fn stop_pending_open(
    cause: OpenStop,
    deadline: tokio::time::Instant,
    tx: &mpsc::UnboundedSender<Packet>,
    room: &mut Option<mpsc::UnboundedSender<RoomEvent>>,
    observed: &mut mpsc::UnboundedReceiver<Terminal>,
    presence_now: &std::sync::Arc<std::sync::Mutex<Instant>>,
) {
    tokio::time::pause();
    match cause {
        OpenStop::End => control(tx, serde_json::json!({"type":"end_interview"})),
        OpenStop::Deadline => {
            // Account for Tokio's millisecond timer resolution at the deadline.
            tokio::time::advance(
                deadline.saturating_duration_since(tokio::time::Instant::now())
                    + Duration::from_millis(1),
            )
            .await;
        }
        OpenStop::RoomDisconnected => room
            .as_ref()
            .unwrap()
            .send(RoomEvent::Disconnected {
                reason: DisconnectReason::ClientInitiated,
            })
            .unwrap(),
        OpenStop::EventsEof => drop(room.take().unwrap()),
        OpenStop::Absent => {
            let mut now = presence_now.lock().unwrap();
            *now += CANDIDATE_ABSENCE_LIMIT;
            drop(now);
            tx.send(Packet {
                topic: Some(TOPIC_CONTROL.into()),
                sender: Some("bystander".into()),
                payload: b"{}".to_vec(),
            })
            .unwrap();
        }
    }
    let frozen = tokio::time::Instant::now();
    let mut terminal = None;
    for _ in 0..4096 {
        if let Ok(value) = observed.try_recv() {
            terminal = Some(value);
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(tokio::time::Instant::now(), frozen);
    tokio::time::resume();
    assert!(
        matches!(
            (cause, terminal),
            (OpenStop::End, Some(Terminal::End { .. }))
                | (OpenStop::Absent, Some(Terminal::Absent))
                | (OpenStop::Deadline, Some(Terminal::Deadline))
                | (
                    OpenStop::RoomDisconnected | OpenStop::EventsEof,
                    Some(Terminal::RoomClosed)
                )
        ),
        "the terminal must win before the held operation or timeout completes: {cause:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn terminal_matrix_joins_readers_at_pending_open_boundaries() {
    if isolated_checked(
        "terminal_matrix_joins_readers_at_pending_open_boundaries",
        |output| {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let usage = stderr
                .lines()
                .filter(|line| line.starts_with("codetrial live_turn_usage "))
                .collect::<Vec<_>>();
            let mut expected = Vec::new();
            for after_setup in [false, false, false, true, false, false, false, true] {
                for _ in 0..5 {
                    expected.push((1, "tool", 11, 1));
                    if after_setup {
                        expected.push((2, "candidate", 29, 2));
                    }
                }
            }
            assert_eq!(usage.len(), 50);
            for (line, (socket, cause, prompt, event)) in usage.into_iter().zip(expected) {
                for field in [
                    format!("socket={socket}"),
                    format!("cause={cause}"),
                    format!("prompt_tokens={prompt}"),
                    format!("usage_event={event}"),
                ] {
                    assert!(
                        line.split_whitespace().any(|value| value == field),
                        "{line}"
                    );
                }
            }
        },
    ) {
        return;
    }
    let mut rows = 0;
    for checkpoint in [false, true] {
        for phase in [
            None,
            Some(PendingOpen::Upgrade),
            Some(PendingOpen::Setup),
            Some(PendingOpen::ReconnectedNotice),
        ] {
            for cause in [
                OpenStop::End,
                OpenStop::Deadline,
                OpenStop::RoomDisconnected,
                OpenStop::EventsEof,
                OpenStop::Absent,
            ] {
                let (listener, endpoint) = listener().await;
                let mut unopened = Some(listener);
                let mut rig = Rig::with_old_checkpoint(endpoint, Some(11), checkpoint).await;
                rig.gemini.input_cause = Some("tool");
                let old = rig.gemini.inspect_reader_for_test().unwrap();
                wait_for_received_usage(&old).await;
                assert_eq!(rig.gemini.recovery_handle(&rig.keys).is_some(), checkpoint);
                rig.loops.deadline_at = tokio::time::Instant::now() + Duration::from_secs(8);
                let deadline = rig.loops.deadline_at;
                let presence_now = std::sync::Arc::new(std::sync::Mutex::new(Instant::now()));
                if matches!(cause, OpenStop::Absent) {
                    rig.loops.presence.left(*presence_now.lock().unwrap());
                    rig.loops.presence.test_now = Some(presence_now.clone());
                }
                let after_setup = matches!(phase, Some(PendingOpen::ReconnectedNotice));
                let (probe, mut observed) = mpsc::unbounded_channel();
                rig.loops.recovery_terminal = Some(probe);
                let (phase_probe, mut phases) = mpsc::unbounded_channel();
                rig.loops.recovery_phase = Some(phase_probe);
                let (reader, mut readers) = mpsc::unbounded_channel();
                rig.loops.recovery_reader = Some(reader);
                let (notice, _release) = rig
                    .recorder
                    .hold_data("interviewer_state", Some(phase.is_none()));
                let (ready, entered) = oneshot::channel();
                let (finish, finished) = oneshot::channel();
                let instructions =
                    crate::runtime::bootstrap(&rig.config, "recovery-test", Some("two-sum"), 45)
                        .instructions;
                let server = phase.map(|phase| {
                    let listener = unopened.take().unwrap();
                    tokio::spawn(async move {
                        let (mut socket, _) = listener.accept().await.unwrap();
                        if matches!(phase, PendingOpen::Upgrade) {
                            use tokio::io::AsyncReadExt;
                            let mut request = Vec::new();
                            while !request.ends_with(b"\r\n\r\n") {
                                assert!(request.len() < 4096);
                                request.push(socket.read_u8().await.unwrap());
                            }
                            ready.send(()).unwrap();
                            closed_tcp(socket).await;
                        } else {
                            let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
                            let setup = socket.next().await.unwrap().unwrap();
                            let setup: serde_json::Value =
                                serde_json::from_str(setup.to_text().unwrap()).unwrap();
                            assert_eq!(
                                setup["setup"]["systemInstruction"]["parts"][0]["text"],
                                instructions
                            );
                            if checkpoint {
                                assert_eq!(
                                    setup["setup"]["sessionResumption"]["handle"],
                                    "matrix-checkpoint"
                                );
                            } else {
                                assert!(setup["setup"]["sessionResumption"]["handle"].is_null());
                            }
                            if after_setup {
                                socket
                                    .send(Message::Text(r#"{"setupComplete":{}}"#.into()))
                                    .await
                                    .unwrap();
                                socket
                                    .send(Message::Text(
                                        r#"{"usageMetadata":{"promptTokenCount":29}}"#.into(),
                                    ))
                                    .await
                                    .unwrap();
                            }
                            ready.send(()).unwrap();
                            closed_tcp(socket.into_inner()).await;
                        }
                        finished.await.unwrap();
                        assert_eq!(
                            listener.into_std().unwrap().accept().unwrap_err().kind(),
                            std::io::ErrorKind::WouldBlock
                        );
                    })
                });

                // Move the sole sender out of the rig. EOF must be a real
                // receiver closure, not a Disconnected event pretending to be
                // one.
                let (unused, _) = mpsc::unbounded_channel();
                let mut room = Some(std::mem::replace(&mut rig._room_sender, unused));
                let tx = rig.tx.clone();
                let fresh_state =
                    std::cell::RefCell::new(None::<crate::gemini::LiveReaderInspection>);
                let running = async {
                    let flow = rig.run().await;
                    assert!(
                        old.is_finished(),
                        "old reader joined in the return poll: {phase:?} {cause:?}"
                    );
                    assert_eq!(old.pending_usage_count(), 0);
                    let fresh = fresh_state.borrow();
                    assert_eq!(fresh.is_some(), after_setup);
                    if let Some(fresh) = fresh.as_ref() {
                        assert!(
                            fresh.is_finished(),
                            "rejected fresh reader joined in the return poll: {cause:?}"
                        );
                        assert_eq!(fresh.pending_usage_count(), 0);
                    }
                    flow
                };
                let stimulus = async {
                    if phase.is_some() {
                        entered.await.unwrap();
                    }
                    if phase.is_none() || after_setup {
                        notice.notified().await;
                    }
                    if after_setup {
                        let fresh = readers.recv().await.unwrap();
                        wait_for_received_usage(&fresh).await;
                        assert!(!fresh.is_finished());
                        fresh_state.replace(Some(fresh));
                    }
                    stop_pending_open(
                        cause,
                        deadline,
                        &tx,
                        &mut room,
                        &mut observed,
                        &presence_now,
                    )
                    .await;
                };
                let (flow, ()) = tokio::join!(running, stimulus);
                assert!(flow.is_break());
                let entered_phases =
                    std::iter::from_fn(|| phases.try_recv().ok()).collect::<Vec<_>>();
                let expected_phases: &[&str] = if phase.is_none() {
                    &[]
                } else if checkpoint {
                    &["close", "resume"]
                } else {
                    &["close"]
                };
                assert_eq!(
                    entered_phases, expected_phases,
                    "phase={phase:?} checkpoint={checkpoint} cause={cause:?}"
                );
                assert_eq!(rig.loops.deadline_at, deadline);
                assert_eq!(rig.loops.restarts, 1);
                assert_eq!(rig.turn.activity.live_socket, 1 + u64::from(after_setup));
                assert_eq!(
                    rig.turn.state.evidence_ledger.metrics.live_setup_count,
                    u32::from(after_setup)
                );
                assert_eq!(
                    rig.turn.activity.live_usage.samples,
                    if after_setup { 2 } else { 1 }
                );
                assert_eq!(
                    rig.turn.activity.live_usage.prompt,
                    if after_setup { 40 } else { 11 }
                );
                assert!(
                    rig.gemini
                        .send_text("must remain a closed placeholder")
                        .await
                        .is_err()
                );
                assert!(readers.try_recv().is_err());
                assert!(observed.try_recv().is_err());
                assert!(!rig.recorder.take().iter().any(|record| matches!(record,
                room_link::Sent::Data { message, .. } if message["type"] == "interviewer_state" && message["reconnecting"] == false)));
                rig.finish().await;
                if let Some(server) = server {
                    finish.send(()).unwrap();
                    server.await.unwrap();
                }
                if let Some(listener) = unopened {
                    assert_eq!(
                        listener.into_std().unwrap().accept().unwrap_err().kind(),
                        std::io::ErrorKind::WouldBlock
                    );
                }
                rows += 1;
            }
        }
    }
    assert_eq!(rows, 40);
}

#[tokio::test(flavor = "current_thread")]
async fn terminal_matrix_cancels_a_physically_blocked_close() {
    if isolated_checked(
        "terminal_matrix_cancels_a_physically_blocked_close",
        |output| {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let usage = stderr
                .lines()
                .filter(|line| line.starts_with("codetrial live_turn_usage "))
                .collect::<Vec<_>>();
            assert_eq!(usage.len(), 5);
            for line in usage {
                for field in [
                    "socket=1",
                    "cause=tool",
                    "prompt_tokens=11",
                    "usage_event=1",
                ] {
                    assert!(
                        line.split_whitespace().any(|value| value == field),
                        "{line}"
                    );
                }
            }
        },
    ) {
        return;
    }
    let mut rows = 0;
    for cause in [
        OpenStop::End,
        OpenStop::Deadline,
        OpenStop::RoomDisconnected,
        OpenStop::EventsEof,
        OpenStop::Absent,
    ] {
        let (unopened, endpoint) = listener().await;
        let mut rig = Rig::new(endpoint).await;
        rig.gemini.shutdown().await.unwrap();
        rig.old.take().unwrap().await.unwrap();
        let listener = tokio::net::TcpSocket::new_v4().unwrap();
        listener.set_recv_buffer_size(1024).unwrap();
        listener.bind("127.0.0.1:0".parse().unwrap()).unwrap();
        let listener = listener.listen(1).unwrap();
        let endpoint = format!("ws://{}/", listener.local_addr().unwrap());
        let (read, reading) = oneshot::channel();
        let server = tokio::spawn(async move {
            use tokio::io::AsyncReadExt;
            let (socket, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
            socket.next().await.unwrap().unwrap();
            socket
                .send(Message::Text(r#"{"setupComplete":{}}"#.into()))
                .await
                .unwrap();
            socket
                .send(Message::Text(
                    r#"{"usageMetadata":{"promptTokenCount":11}}"#.into(),
                ))
                .await
                .unwrap();
            let mut socket = socket.into_inner();
            reading.await.unwrap();
            let mut bytes = [0; 65536];
            loop {
                match socket.read(&mut bytes).await {
                    Ok(0) => break,
                    Ok(_) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::ConnectionReset => break,
                    other => panic!("cancelled close must release the transport: {other:?}"),
                }
            }
            Vec::new()
        });
        let boot = crate::runtime::bootstrap(&rig.config, "recovery-test", Some("two-sum"), 45);
        rig.gemini = crate::gemini::live_session_with_keys_at(&endpoint, &rig.keys, &boot, None)
            .await
            .unwrap();
        rig.gemini.input_cause = Some("tool");
        let old = rig.gemini.inspect_reader_for_test().unwrap();
        wait_for_received_usage(&old).await;
        rig.old = Some(server);
        let mut blocked = false;
        for _ in 0..64 {
            if tokio::time::timeout(
                Duration::from_millis(30),
                rig.gemini
                    .writer_for_test()
                    .send(Message::Binary(vec![0; 256 * 1024].into())),
            )
            .await
            .is_err()
            {
                blocked = true;
                break;
            }
        }
        assert!(
            blocked,
            "the fixture must establish real writer backpressure"
        );

        // Unlike a before-write gate, the peer is not reading an already
        // stalled writer. The original deadline precedes its 2s close timeout.
        rig.loops.deadline_at = tokio::time::Instant::now() + Duration::from_secs(1);
        let deadline = rig.loops.deadline_at;
        let presence_now = std::sync::Arc::new(std::sync::Mutex::new(Instant::now()));
        if matches!(cause, OpenStop::Absent) {
            rig.loops.presence.left(*presence_now.lock().unwrap());
            rig.loops.presence.test_now = Some(presence_now.clone());
        }
        let (phase, mut phases) = mpsc::unbounded_channel();
        rig.loops.recovery_phase = Some(phase);
        let (probe, mut observed) = mpsc::unbounded_channel();
        rig.loops.recovery_terminal = Some(probe);
        let (reader, mut readers) = mpsc::unbounded_channel();
        rig.loops.recovery_reader = Some(reader);
        let (unused, _) = mpsc::unbounded_channel();
        let mut room = Some(std::mem::replace(&mut rig._room_sender, unused));
        let tx = rig.tx.clone();
        let running = async {
            let flow = rig.run().await;
            assert!(
                old.is_finished(),
                "reader must join in the close return poll: {cause:?}"
            );
            assert_eq!(old.pending_usage_count(), 0);
            flow
        };
        let stimulus = async {
            while phases.recv().await != Some("close") {}
            assert!(
                tokio::time::Instant::now() < deadline,
                "the close barrier must precede the original deadline"
            );
            stop_pending_open(
                cause,
                deadline,
                &tx,
                &mut room,
                &mut observed,
                &presence_now,
            )
            .await;
            read.send(()).unwrap();
        };
        let (flow, ()) = tokio::join!(running, stimulus);
        assert!(flow.is_break());
        assert_eq!(rig.loops.deadline_at, deadline);
        assert_eq!(rig.loops.restarts, 1);
        assert_eq!(rig.turn.activity.live_socket, 1);
        assert_eq!(rig.turn.state.evidence_ledger.metrics.live_setup_count, 0);
        assert_eq!(rig.turn.activity.live_usage.samples, 1);
        assert_eq!(rig.turn.activity.live_usage.prompt, 11);
        assert!(readers.try_recv().is_err());
        assert!(observed.try_recv().is_err());
        assert!(
            rig.gemini
                .send_text("no socket may be installed")
                .await
                .is_err()
        );
        assert!(!rig.recorder.take().iter().any(|record| matches!(record,
            room_link::Sent::Data { message, .. } if message["type"] == "interviewer_state" && message["reconnecting"] == false)));
        rig.finish().await;
        assert_eq!(
            unopened.into_std().unwrap().accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
        rows += 1;
    }
    assert_eq!(rows, 5);
}

#[derive(Clone, Copy, Debug)]
enum PrefixPhase {
    AudioWrite,
    EndWrite,
    InitialCollect,
    Collect,
    NoteGate,
}

const PREFIX_PHASES: [PrefixPhase; 5] = [
    PrefixPhase::AudioWrite,
    PrefixPhase::EndWrite,
    PrefixPhase::InitialCollect,
    PrefixPhase::Collect,
    PrefixPhase::NoteGate,
];
const PREFIX_STOPS: [OpenStop; 5] = [
    OpenStop::End,
    OpenStop::Deadline,
    OpenStop::RoomDisconnected,
    OpenStop::EventsEof,
    OpenStop::Absent,
];
const PREFIX_TEXT: &str = "Keep distinct indices in the answer.";

impl PrefixPhase {
    fn label(self) -> &'static str {
        match self {
            Self::AudioWrite => "prefix_audio_write",
            Self::EndWrite => "prefix_end_audio_write",
            Self::InitialCollect => "prefix_collect_initial",
            Self::Collect => "prefix_collect",
            Self::NoteGate => "prefix_listening_context",
        }
    }

    fn physical(self) -> bool {
        matches!(self, Self::AudioWrite | Self::EndWrite)
    }

    fn transcribed(self) -> bool {
        matches!(self, Self::Collect | Self::NoteGate)
    }
}

async fn prefix_pending_phase(
    phases: &mut mpsc::UnboundedReceiver<&'static str>,
    target: &'static str,
) {
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let phase = phases
                .recv()
                .await
                .expect("the prefix probe must remain owned");
            assert!(
                !matches!(phase, "close" | "resume"),
                "prefix finished before cancellation"
            );
            if phase == target {
                break;
            }
        }
    })
    .await
    .expect("the targeted prefix operation must actually pend");
}

#[tokio::test(flavor = "current_thread")]
async fn prefix_terminal_matrix_preserves_received_evidence_before_any_restart() {
    if isolated_checked(
        "prefix_terminal_matrix_preserves_received_evidence_before_any_restart",
        |output| {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let usage = stderr
                .lines()
                .filter(|line| line.starts_with("codetrial live_turn_usage "))
                .collect::<Vec<_>>();
            let mut expected = Vec::new();
            for phase in PREFIX_PHASES {
                for _ in PREFIX_STOPS {
                    expected.push((11, 1));
                    if phase.transcribed() {
                        expected.push((29, 2));
                    }
                }
            }
            assert_eq!(usage.len(), 35);
            for (line, (prompt, event)) in usage.into_iter().zip(expected) {
                for field in [
                    "socket=1".to_string(),
                    "cause=tool".to_string(),
                    format!("prompt_tokens={prompt}"),
                    format!("usage_event={event}"),
                ] {
                    assert!(
                        line.split_whitespace().any(|value| value == field),
                        "{line}"
                    );
                }
            }
            let ingress = stderr
                .lines()
                .filter(|line| line.starts_with("recovery ingress: "))
                .collect::<Vec<_>>();
            assert_eq!(ingress.len(), 25);
            for (index, line) in ingress.into_iter().enumerate() {
                let uncertain = if PREFIX_PHASES[index / 5].physical() {
                    2
                } else {
                    0
                };
                for field in [
                    "dropped_bytes=0".to_string(),
                    format!("unconfirmed_bytes={uncertain}"),
                    "unsent_bytes=0".to_string(),
                ] {
                    assert!(
                        line.split_whitespace().any(|value| value == field),
                        "{line}"
                    );
                }
            }
            for forbidden in [
                "speech handoff:",
                "recovery setup discarded:",
                "Gemini write stalled",
            ] {
                assert!(
                    !stderr.contains(forbidden),
                    "unexpected completed handoff or timer: {forbidden}"
                );
            }
        },
    ) {
        return;
    }
    let mut rows = 0;
    for phase in PREFIX_PHASES {
        for cause in PREFIX_STOPS {
            eprintln!("prefix matrix: phase={phase:?} cause={cause:?}");
            let (unopened, endpoint) = listener().await;
            let mut rig = Rig::new(endpoint).await;
            rig.gemini.shutdown().await.unwrap();
            rig.old.take().unwrap().await.unwrap();
            let listener = tokio::net::TcpSocket::new_v4().unwrap();
            listener.set_recv_buffer_size(1024).unwrap();
            listener.bind("127.0.0.1:0".parse().unwrap()).unwrap();
            let listener = listener.listen(1).unwrap();
            let endpoint = format!("ws://{}/", listener.local_addr().unwrap());
            let (read, reading) = oneshot::channel();
            let (ready, entered) = oneshot::channel();
            let server = tokio::spawn(async move {
                use tokio::io::AsyncReadExt;
                let (socket, _) = listener.accept().await.unwrap();
                let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
                socket.next().await.unwrap().unwrap();
                socket
                    .send(Message::Text(r#"{"setupComplete":{}}"#.into()))
                    .await
                    .unwrap();
                socket
                    .send(Message::Text(
                        r#"{"usageMetadata":{"promptTokenCount":11}}"#.into(),
                    ))
                    .await
                    .unwrap();
                if phase.physical() {
                    let mut socket = socket.into_inner();
                    reading.await.unwrap();
                    let mut bytes = [0; 65536];
                    loop {
                        match socket.read(&mut bytes).await {
                            Ok(0) => break,
                            Ok(_) => {}
                            Err(error) if error.kind() == std::io::ErrorKind::ConnectionReset => {
                                break;
                            }
                            other => {
                                panic!("cancelled prefix must release stalled transport: {other:?}")
                            }
                        }
                    }
                    return Vec::new();
                }
                let end = socket.next().await.unwrap().unwrap();
                let end: serde_json::Value = serde_json::from_str(end.to_text().unwrap()).unwrap();
                assert_eq!(end["realtimeInput"]["audioStreamEnd"], true);
                let mut messages = vec![end];
                if phase.transcribed() {
                    socket.send(Message::Text(serde_json::json!({"serverContent":{"inputTranscription":{"text":PREFIX_TEXT}},"usageMetadata":{"promptTokenCount":29}}).to_string().into())).await.unwrap();
                    if matches!(phase, PrefixPhase::Collect) {
                        let note = socket.next().await.unwrap().unwrap();
                        let note: serde_json::Value =
                            serde_json::from_str(note.to_text().unwrap()).unwrap();
                        assert_eq!(note["clientContent"]["turnComplete"], false);
                        assert!(note.to_string().contains("Keep listening"));
                        messages.push(note);
                    }
                }
                ready.send(()).unwrap();
                closed_tcp(socket.into_inner()).await;
                messages
            });
            let boot = crate::runtime::bootstrap(&rig.config, "recovery-test", Some("two-sum"), 45);
            rig.gemini =
                crate::gemini::live_session_with_keys_at(&endpoint, &rig.keys, &boot, None)
                    .await
                    .unwrap();
            rig.gemini.input_cause = Some("tool");
            let old = rig.gemini.inspect_reader_for_test().unwrap();
            wait_for_received_usage(&old).await;
            rig.old = Some(server);
            rig.expected_old_messages = if phase.physical() {
                0
            } else if matches!(phase, PrefixPhase::Collect) {
                2
            } else {
                1
            };
            if phase.physical() {
                let mut blocked = false;
                for _ in 0..64 {
                    if tokio::time::timeout(
                        Duration::from_millis(30),
                        rig.gemini
                            .writer_for_test()
                            .send(Message::Binary(vec![0; 256 * 1024].into())),
                    )
                    .await
                    .is_err()
                    {
                        blocked = true;
                        break;
                    }
                }
                assert!(
                    blocked,
                    "the prefix writer must already be physically stalled"
                );
                rig.media.audio_bytes = vec![1, 2];
                rig.media.audio_unconfirmed = matches!(phase, PrefixPhase::EndWrite);
            }
            rig.prefix = true;
            rig.media.identity = Some("candidate".into());
            rig.loops.deferred_restart = DeferredRestart(true);
            rig.turn
                .activity
                .note_candidate_voice_activity(true, Instant::now());
            let gate = if phase.transcribed() {
                // An unrelated earlier gate must survive the tagged note send.
                rig.loops.recovery_writes.push_back(WriteBarrier {
                    phase: "recovery_brief",
                    entered: std::sync::Arc::new(tokio::sync::Notify::new()),
                    release: std::sync::Arc::new(tokio::sync::Notify::new()),
                });
                let entered = std::sync::Arc::new(tokio::sync::Notify::new());
                let release = std::sync::Arc::new(tokio::sync::Notify::new());
                rig.loops.recovery_writes.push_back(WriteBarrier {
                    phase: "prefix_listening_context",
                    entered: entered.clone(),
                    release: release.clone(),
                });
                Some((entered, release))
            } else {
                None
            };
            rig.loops.deadline_at = tokio::time::Instant::now()
                + Duration::from_secs(if phase.transcribed() { 8 } else { 1 });
            let deadline = rig.loops.deadline_at;
            let presence_now = std::sync::Arc::new(std::sync::Mutex::new(Instant::now()));
            if matches!(cause, OpenStop::Absent) {
                rig.loops.presence.left(*presence_now.lock().unwrap());
                rig.loops.presence.test_now = Some(presence_now.clone());
            }
            let (probe, mut observed) = mpsc::unbounded_channel();
            rig.loops.recovery_terminal = Some(probe);
            let (phase_probe, mut phases) = mpsc::unbounded_channel();
            rig.loops.recovery_phase = Some(phase_probe);
            let (reader, mut readers) = mpsc::unbounded_channel();
            rig.loops.recovery_reader = Some(reader);
            let (unused, _) = mpsc::unbounded_channel();
            let mut room = Some(std::mem::replace(&mut rig._room_sender, unused));
            let tx = rig.tx.clone();
            let running = async {
                let flow = rig.run().await;
                assert!(
                    old.is_finished(),
                    "prefix reader must join in return poll: {phase:?} {cause:?}"
                );
                assert_eq!(old.pending_usage_count(), 0);
                flow
            };
            let stimulus = async {
                if let Some((gate_entered, release)) = &gate {
                    tokio::time::timeout(Duration::from_secs(1), gate_entered.notified())
                        .await
                        .expect("the matching prefix note gate must be entered");
                    if matches!(phase, PrefixPhase::Collect) {
                        if matches!(cause, OpenStop::Deadline) {
                            tokio::time::pause();
                            let left =
                                deadline.saturating_duration_since(tokio::time::Instant::now());
                            tokio::time::advance(
                                left.checked_sub(Duration::from_millis(190)).unwrap(),
                            )
                            .await;
                            tokio::time::resume();
                        }
                        release.notify_one();
                    }
                }
                if !phase.physical() {
                    entered.await.unwrap();
                }
                prefix_pending_phase(&mut phases, phase.label()).await;
                assert!(tokio::time::Instant::now() < deadline);
                assert_eq!(
                    old.pending_usage_count(),
                    if phase.transcribed() { 2 } else { 1 }
                );
                assert!(!old.is_finished());
                stop_pending_open(
                    cause,
                    deadline,
                    &tx,
                    &mut room,
                    &mut observed,
                    &presence_now,
                )
                .await;
                if phase.physical() {
                    read.send(()).unwrap();
                }
            };
            let (flow, ()) = tokio::join!(running, stimulus);
            assert!(flow.is_break());
            assert_eq!(rig.loops.deadline_at, deadline);
            assert_eq!(rig.loops.restarts, 0);
            assert_eq!(rig.turn.activity.live_socket, 1);
            assert_eq!(rig.turn.state.evidence_ledger.metrics.live_setup_count, 0);
            assert_eq!(
                rig.turn.activity.live_usage.samples,
                if phase.transcribed() { 2 } else { 1 }
            );
            assert_eq!(
                rig.turn.activity.live_usage.prompt,
                if phase.transcribed() { 40 } else { 11 }
            );
            assert_eq!(
                rig.turn.state.evidence_ledger.metrics.turn_prompt_count,
                u32::from(phase.transcribed())
            );
            assert_eq!(
                rig.turn
                    .state
                    .transcript
                    .iter()
                    .filter(|line| line.contains(PREFIX_TEXT))
                    .count(),
                usize::from(phase.transcribed())
            );
            assert!(
                rig.gemini
                    .send_text("no socket may be installed")
                    .await
                    .is_err()
            );
            assert_eq!(
                rig.loops.recovery_writes.len(),
                usize::from(phase.transcribed())
            );
            if let Some(unused) = rig.loops.recovery_writes.front() {
                assert_eq!(unused.phase, "recovery_brief");
            }
            assert!(readers.try_recv().is_err());
            assert!(observed.try_recv().is_err());
            assert!(
                phases.try_recv().is_err(),
                "no operation after the target may pend"
            );
            rig.turn.transcripts.flush(Duration::from_secs(1)).await;
            assert_eq!(rig.recorder.take().iter().filter(|record| matches!(record,
                room_link::Sent::Text { topic, text, .. } if topic == crate::runtime::TOPIC_TRANSCRIPTION && text == PREFIX_TEXT)).count(), usize::from(phase.transcribed()));
            rig.finish().await;
            assert_eq!(
                unopened.into_std().unwrap().accept().unwrap_err().kind(),
                std::io::ErrorKind::WouldBlock
            );
            rows += 1;
        }
    }
    assert_eq!(rows, 25);
}

#[tokio::test(flavor = "current_thread")]
async fn a_ready_retry_session_remains_owned_when_its_attempt_notice_is_cancelled() {
    if isolated("a_ready_retry_session_remains_owned_when_its_attempt_notice_is_cancelled") {
        return;
    }
    let config = load_from_pairs([
        ("LIVEKIT_URL", "wss://example.livekit.cloud"),
        ("LIVEKIT_API_KEY", "devkey"),
        ("LIVEKIT_API_SECRET", "devsecret"),
        ("GOOGLE_API_KEY", "retry-owned-test-key"),
    ])
    .unwrap();
    let keys = GeminiKeys::from_config(&config);
    let boot = crate::runtime::bootstrap(&config, "retry-owned", Some("two-sum"), 45);
    let (listener, endpoint) = listener().await;
    let accepts = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let seen_accepts = accepts.clone();
    let instructions = boot.instructions.clone();
    let (finish, finished) = oneshot::channel();
    let server = tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let (mut socket, _) = listener.accept().await.unwrap();
        seen_accepts.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let mut request = Vec::new();
        while !request.ends_with(b"\r\n\r\n") {
            assert!(request.len() < 4096);
            request.push(socket.read_u8().await.unwrap());
        }
        socket.write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await.unwrap();
        drop(socket);
        let (socket, _) = listener.accept().await.unwrap();
        seen_accepts.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
        let setup = socket.next().await.unwrap().unwrap();
        let setup: serde_json::Value = serde_json::from_str(setup.to_text().unwrap()).unwrap();
        assert_eq!(
            setup["setup"]["systemInstruction"]["parts"][0]["text"],
            instructions
        );
        assert!(setup["setup"]["sessionResumption"]["handle"].is_null());
        socket
            .send(Message::Text(r#"{"setupComplete":{}}"#.into()))
            .await
            .unwrap();
        socket
            .send(Message::Text(
                r#"{"usageMetadata":{"promptTokenCount":29}}"#.into(),
            ))
            .await
            .unwrap();
        closed_tcp(socket.into_inner()).await;
        finished.await.unwrap();
        assert_eq!(
            listener.into_std().unwrap().accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    });
    let (reader, mut readers) = mpsc::unbounded_channel();
    let notice = std::sync::Arc::new(tokio::sync::Notify::new());
    let cancel = std::sync::Arc::new(tokio::sync::Notify::new());
    let inspection = std::cell::RefCell::new(None::<crate::gemini::LiveReaderInspection>);
    let mut restarts = 0;
    let mut ready = None;
    let running = async {
        {
            let opening = retry_cold_open_into(
                &keys,
                &mut restarts,
                &mut ready,
                || {
                    let reader = reader.clone();
                    let endpoint = &endpoint;
                    let keys = &keys;
                    let boot = &boot;
                    async move {
                        let opened =
                            crate::gemini::live_session_with_keys_at(endpoint, keys, boot, None)
                                .await;
                        if let Ok(session) = &opened {
                            reader
                                .send(session.inspect_reader_for_test().unwrap())
                                .unwrap();
                        }
                        opened
                    }
                },
                |message| {
                    let notice = notice.clone();
                    async move {
                        if message.get("waitSeconds").is_none() {
                            notice.notify_one();
                            std::future::pending::<()>().await;
                        }
                    }
                },
                tokio::time::sleep_until,
            );
            tokio::pin!(opening);
            tokio::select! {
                biased;
                () = cancel.notified() => {}
                result = &mut opening => panic!("the attempt notice must remain pending: {result:?}"),
            }
        }

        // The future has been cancelled, but its completed transport belongs to
        // this caller. Dropping a join's buffered result would hide it.
        let mut session = ready
            .take()
            .expect("a completed retry setup must stay in the caller-owned slot");
        assert_eq!(restarts, 1, "one retry charge after the initial 503");
        assert_eq!(accepts.load(std::sync::atomic::Ordering::SeqCst), 2);
        session.stop_reader().await;
        let usage = session.drain_usage();
        assert_eq!(usage.len(), 1);
        assert_eq!(usage[0].prompt, 29);
        let inspected = inspection.borrow();
        let inspected = inspected.as_ref().unwrap();
        assert!(inspected.is_finished());
        assert_eq!(inspected.pending_usage_count(), 0);
        drop(session);
    };
    let stimulus = async {
        notice.notified().await;
        let observed = readers.recv().await.unwrap();
        wait_for_received_usage(&observed).await;
        assert!(!observed.is_finished());
        assert_eq!(accepts.load(std::sync::atomic::Ordering::SeqCst), 2);
        inspection.replace(Some(observed));
        cancel.notify_one();
    };
    tokio::time::timeout(Duration::from_secs(8), async {
        tokio::join!(running, stimulus);
    })
    .await
    .unwrap();
    finish.send(()).unwrap();
    server.await.unwrap();
}

#[derive(Clone, Copy, Debug)]
enum RetryWait {
    WaitNotice,
    Backoff,
    AttemptPending,
    AttemptReady,
}

impl RetryWait {
    fn second_attempt(self) -> bool {
        matches!(self, Self::AttemptPending | Self::AttemptReady)
    }
    fn ready(self) -> bool {
        matches!(self, Self::AttemptReady)
    }
}

fn retry_usage(output: &std::process::Output, ready: bool) {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let lines = stderr
        .lines()
        .filter(|line| line.starts_with("codetrial live_turn_usage "))
        .collect::<Vec<_>>();
    assert_eq!(lines.len(), if ready { 10 } else { 5 });
    let mut expected = Vec::new();
    for _ in PREFIX_STOPS {
        expected.push((1, "tool", 11, 1));
        if ready {
            expected.push((2, "candidate", 29, 2));
        }
    }
    for (line, (socket, cause, prompt, event)) in lines.into_iter().zip(expected) {
        for field in [
            format!("socket={socket}"),
            format!("cause={cause}"),
            format!("prompt_tokens={prompt}"),
            format!("usage_event={event}"),
        ] {
            assert!(
                line.split_whitespace().any(|value| value == field),
                "{line}"
            );
        }
    }
}

async fn retry_wait_case_matrix(phase: RetryWait) {
    let mut rows = 0;
    for cause in PREFIX_STOPS {
        let (listener, endpoint) = listener().await;
        let mut rig = Rig::with_old_usage(endpoint, Some(11)).await;
        rig.gemini.input_cause = Some("tool");
        let old = rig.gemini.inspect_reader_for_test().unwrap();
        wait_for_received_usage(&old).await;
        let accepts = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let server_accepts = accepts.clone();
        let instructions =
            crate::runtime::bootstrap(&rig.config, "recovery-test", Some("two-sum"), 45)
                .instructions;
        let (accepted, second_accepted) = oneshot::channel();
        let (finish, finished) = oneshot::channel();
        let server = tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let (mut socket, _) = listener.accept().await.unwrap();
            server_accepts.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") {
                assert!(request.len() < 4096);
                request.push(socket.read_u8().await.unwrap());
            }
            socket.write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await.unwrap();
            drop(socket);
            if phase.second_attempt() {
                let (mut socket, _) = listener.accept().await.unwrap();
                server_accepts.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                if phase.ready() {
                    let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
                    let setup = socket.next().await.unwrap().unwrap();
                    let setup: serde_json::Value =
                        serde_json::from_str(setup.to_text().unwrap()).unwrap();
                    assert_eq!(
                        setup["setup"]["systemInstruction"]["parts"][0]["text"],
                        instructions
                    );
                    assert!(setup["setup"]["sessionResumption"]["handle"].is_null());
                    socket
                        .send(Message::Text(r#"{"setupComplete":{}}"#.into()))
                        .await
                        .unwrap();
                    socket
                        .send(Message::Text(
                            r#"{"usageMetadata":{"promptTokenCount":29}}"#.into(),
                        ))
                        .await
                        .unwrap();
                    accepted.send(()).unwrap();
                    closed_tcp(socket.into_inner()).await;
                } else {
                    let mut request = Vec::new();
                    while !request.ends_with(b"\r\n\r\n") {
                        assert!(request.len() < 4096);
                        request.push(socket.read_u8().await.unwrap());
                    }
                    accepted.send(()).unwrap();
                    closed_tcp(socket).await;
                }
            }
            finished.await.unwrap();
            assert_eq!(
                listener.into_std().unwrap().accept().unwrap_err().kind(),
                std::io::ErrorKind::WouldBlock
            );
        });
        let gate = if matches!(phase, RetryWait::WaitNotice) {
            Some(rig.recorder.hold_retry_notice(true))
        } else if phase.second_attempt() {
            Some(rig.recorder.hold_retry_notice(false))
        } else {
            None
        };
        rig.loops.deadline_at = tokio::time::Instant::now()
            + if phase.second_attempt() {
                COLD_OPEN_BACKOFF + RECONNECT_NOTICE_LIMIT - Duration::from_millis(100)
            } else {
                Duration::from_millis(500)
            };
        let deadline = rig.loops.deadline_at;
        let presence_now = std::sync::Arc::new(std::sync::Mutex::new(Instant::now()));
        if matches!(cause, OpenStop::Absent) {
            rig.loops.presence.left(*presence_now.lock().unwrap());
            rig.loops.presence.test_now = Some(presence_now.clone());
        }
        let (probe, mut observed) = mpsc::unbounded_channel();
        rig.loops.recovery_terminal = Some(probe);
        let (reader, mut readers) = mpsc::unbounded_channel();
        rig.loops.recovery_reader = Some(reader);
        let (unused, _) = mpsc::unbounded_channel();
        let mut room = Some(std::mem::replace(&mut rig._room_sender, unused));
        let tx = rig.tx.clone();
        let recorder = rig.recorder.clone();
        let fresh_state = std::cell::RefCell::new(None::<crate::gemini::LiveReaderInspection>);
        let running = async {
            let result = rig.run().await;
            assert!(old.is_finished());
            assert_eq!(old.pending_usage_count(), 0);
            let fresh = fresh_state.borrow();
            assert_eq!(fresh.is_some(), phase.ready());
            if let Some(fresh) = fresh.as_ref() {
                assert!(
                    fresh.is_finished(),
                    "internally ready retry reader must join before return: {cause:?}"
                );
                assert_eq!(fresh.pending_usage_count(), 0);
            }
            result
        };
        let stimulus = async {
            if !matches!(phase, RetryWait::WaitNotice) {
                let until = tokio::time::timeout(Duration::from_secs(1), async {
                    loop {
                        let mut deadlines = recorder.take_retry_backoffs();
                        if let Some(until) = deadlines.pop() {
                            assert!(deadlines.is_empty());
                            break until;
                        }
                        tokio::task::yield_now().await;
                    }
                })
                .await
                .unwrap();
                if phase.second_attempt() {
                    // Stop one timer tick before backoff expires, then resume
                    // before the next real socket can start doing I/O.
                    tokio::time::pause();
                    tokio::time::advance(
                        until
                            .saturating_duration_since(tokio::time::Instant::now())
                            .saturating_sub(Duration::from_millis(1)),
                    )
                    .await;
                    tokio::time::resume();
                } else {
                    assert!(deadline < until);
                }
            }
            if let Some((entered, _)) = &gate {
                tokio::time::timeout(Duration::from_secs(1), entered.notified())
                    .await
                    .unwrap();
            }
            if phase.second_attempt() {
                second_accepted.await.unwrap();
            }
            if phase.ready() {
                let fresh = tokio::time::timeout(Duration::from_secs(1), async {
                    loop {
                        let mut created = recorder.take_live_readers();
                        if let Some(reader) = created.pop() {
                            assert!(created.is_empty());
                            break reader;
                        }
                        tokio::task::yield_now().await;
                    }
                })
                .await
                .unwrap();
                wait_for_received_usage(&fresh).await;
                assert!(!fresh.is_finished());
                fresh_state.replace(Some(fresh));
            }
            assert!(old.is_finished());
            assert_eq!(old.pending_usage_count(), 0);
            assert!(tokio::time::Instant::now() < deadline);
            assert_eq!(
                accepts.load(std::sync::atomic::Ordering::SeqCst),
                if phase.second_attempt() { 2 } else { 1 }
            );
            stop_pending_open(
                cause,
                deadline,
                &tx,
                &mut room,
                &mut observed,
                &presence_now,
            )
            .await;
        };
        let (flow, ()) = tokio::join!(running, stimulus);
        assert!(flow.is_break());
        assert_eq!(rig.loops.deadline_at, deadline);
        assert_eq!(rig.loops.restarts, 2);
        assert_eq!(rig.turn.activity.live_socket, 1 + u64::from(phase.ready()));
        assert_eq!(
            rig.turn.state.evidence_ledger.metrics.live_setup_count,
            u32::from(phase.ready())
        );
        assert_eq!(
            rig.turn.activity.live_usage.samples,
            if phase.ready() { 2 } else { 1 }
        );
        assert_eq!(
            rig.turn.activity.live_usage.prompt,
            if phase.ready() { 40 } else { 11 }
        );
        assert!(
            rig.gemini
                .send_text("no retry socket may be installed")
                .await
                .is_err()
        );
        if phase.ready() {
            let ready = readers
                .try_recv()
                .expect("the completed setup was charged even on cancellation");
            assert!(ready.is_finished());
            assert_eq!(ready.pending_usage_count(), 0);
        }
        assert!(readers.try_recv().is_err());
        assert!(observed.try_recv().is_err());
        assert!(rig.recorder.take_live_readers().is_empty());
        assert!(!rig.recorder.take().iter().any(|record|matches!(record,room_link::Sent::Data{message,..} if message["type"]=="interviewer_state"&&message["reconnecting"]==false)));
        rig.finish().await;
        finish.send(()).unwrap();
        server.await.unwrap();
        rows += 1;
    }
    assert_eq!(rows, 5);
}

#[tokio::test(flavor = "current_thread")]
async fn cold_retry_wait_notice_remains_cancellable() {
    if isolated_checked("cold_retry_wait_notice_remains_cancellable", |out| {
        retry_usage(out, false)
    }) {
        return;
    }
    retry_wait_case_matrix(RetryWait::WaitNotice).await;
}
#[tokio::test(flavor = "current_thread")]
async fn cold_retry_backoff_remains_cancellable() {
    if isolated_checked("cold_retry_backoff_remains_cancellable", |out| {
        retry_usage(out, false)
    }) {
        return;
    }
    retry_wait_case_matrix(RetryWait::Backoff).await;
}
#[tokio::test(flavor = "current_thread")]
async fn cold_retry_attempt_notice_cancels_pending_open() {
    if isolated_checked("cold_retry_attempt_notice_cancels_pending_open", |out| {
        retry_usage(out, false)
    }) {
        return;
    }
    retry_wait_case_matrix(RetryWait::AttemptPending).await;
}
#[tokio::test(flavor = "current_thread")]
async fn cold_retry_attempt_notice_owns_completed_setup() {
    if isolated_checked("cold_retry_attempt_notice_owns_completed_setup", |out| {
        retry_usage(out, true)
    }) {
        return;
    }
    retry_wait_case_matrix(RetryWait::AttemptReady).await;
}

#[tokio::test(flavor = "current_thread")]
async fn queued_terminals_preempt_the_replacement_entry_before_any_charge() {
    if isolated_checked(
        "queued_terminals_preempt_the_replacement_entry_before_any_charge",
        |output| {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let usage = stderr
                .lines()
                .filter(|line| line.starts_with("codetrial live_turn_usage "))
                .collect::<Vec<_>>();
            assert_eq!(usage.len(), 5);
            for line in usage {
                for field in [
                    "socket=1",
                    "usage_event=1",
                    "cause=tool",
                    "prompt_tokens=11",
                ] {
                    assert!(
                        line.split_whitespace().any(|value| value == field),
                        "{line}"
                    );
                }
            }
        },
    ) {
        return;
    }
    let mut rows = 0;
    for cause in PREFIX_STOPS {
        let (unopened, endpoint) = listener().await;
        let mut rig = Rig::with_old_usage(endpoint, Some(11)).await;
        rig.gemini.input_cause = Some("tool");
        let old = rig.gemini.inspect_reader_for_test().unwrap();
        wait_for_received_usage(&old).await;
        let (reader, mut readers) = mpsc::unbounded_channel();
        rig.loops.recovery_reader = Some(reader);
        let (phase, mut phases) = mpsc::unbounded_channel();
        rig.loops.recovery_phase = Some(phase);
        rig.loops.deadline_at = tokio::time::Instant::now() + Duration::from_secs(8);
        let presence_now = std::sync::Arc::new(std::sync::Mutex::new(Instant::now()));
        let end = serde_json::json!({"type":"end_interview", "reason":"entry-marker", "code":"fn final_entry_marker() {}", "language":"rust"});
        match cause {
            OpenStop::End => control(&rig.tx, end.clone()),
            OpenStop::Deadline => rig.loops.deadline_at = tokio::time::Instant::now(),
            OpenStop::RoomDisconnected => rig
                ._room_sender
                .send(RoomEvent::Disconnected {
                    reason: DisconnectReason::ClientInitiated,
                })
                .unwrap(),
            OpenStop::EventsEof => {
                let (unused, _) = mpsc::unbounded_channel();
                drop(std::mem::replace(&mut rig._room_sender, unused));
            }
            OpenStop::Absent => {
                rig.loops.presence.left(*presence_now.lock().unwrap());
                rig.loops.presence.test_now = Some(presence_now.clone());
                *presence_now.lock().unwrap() += CANDIDATE_ABSENCE_LIMIT;
            }
        }
        let deadline = rig.loops.deadline_at;
        let (terminal, mut observed) = mpsc::unbounded_channel();
        rig.loops.recovery_terminal = Some(terminal);
        let code = rig.turn.state.code.clone();
        tokio::time::pause();
        let frozen = tokio::time::Instant::now();
        let running = async {
            let flow = rig.run().await;
            assert!(
                old.is_finished(),
                "old entry reader must join in the return poll: {cause:?}"
            );
            assert_eq!(old.pending_usage_count(), 0);
            flow
        };
        let observation = async {
            // Keep the scheduler runnable: an idle paused runtime must not
            // advance to a transport timeout to make the fixture succeed.
            for _ in 0..4096 {
                if let Ok(terminal) = observed.try_recv() {
                    return terminal;
                }
                tokio::task::yield_now().await;
            }
            panic!("queued entry terminal was not dispatched: {cause:?}");
        };
        let (flow, terminal) = tokio::join!(running, observation);
        assert_eq!(tokio::time::Instant::now(), frozen);
        tokio::time::resume();
        assert!(flow.is_break());
        match (cause, terminal) {
            (OpenStop::End, Terminal::End { topic, payload }) => {
                assert_eq!(topic, TOPIC_CONTROL);
                assert_eq!(
                    payload, end,
                    "the ending owner needs the original final-code packet"
                );
            }
            (OpenStop::Deadline, Terminal::Deadline)
            | (OpenStop::Absent, Terminal::Absent)
            | (OpenStop::RoomDisconnected | OpenStop::EventsEof, Terminal::RoomClosed) => {}
            other => panic!("wrong entry terminal: {other:?}"),
        }
        assert_eq!(rig.loops.deadline_at, deadline);
        assert_eq!(
            rig.loops.restarts, 0,
            "entry terminal cannot admit a restart"
        );
        assert_eq!(rig.turn.activity.live_socket, 1);
        assert_eq!(rig.turn.state.evidence_ledger.metrics.live_setup_count, 0);
        assert_eq!(rig.turn.activity.live_usage.samples, 1);
        assert_eq!(rig.turn.activity.live_usage.prompt, 11);
        assert_eq!(rig.turn.state.code, code);
        assert!(
            !rig.turn.state.ended,
            "only the real ending owner reduces End"
        );
        assert!(
            rig.gemini
                .send_text("no entry replacement installed")
                .await
                .is_err()
        );
        assert!(readers.try_recv().is_err());
        assert!(phases.try_recv().is_err());
        assert!(observed.try_recv().is_err());
        assert!(!rig.recorder.take().iter().any(|sent| matches!(sent, room_link::Sent::Data { message, .. } if message["type"] == "interviewer_state")));
        assert_eq!(
            unopened.into_std().unwrap().accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
        rig.finish().await;
        rows += 1;
    }
    assert_eq!(rows, 5);
}

#[tokio::test(flavor = "current_thread")]
async fn end_is_intercepted_before_its_finish_producing_reducer() {
    if isolated("end_is_intercepted_before_its_finish_producing_reducer") {
        return;
    }
    let payload = serde_json::json!({"type":"end_interview", "reason":"finish-producer-marker", "code":"fn final_reducer_marker() {}", "language":"rust"});
    let mut rig = Rig::new("ws://127.0.0.1:9/".into()).await;
    let original = rig.turn.state.clone();
    let mut normally_reduced = original.clone();
    let reduced =
        crate::agent::apply_data_event(&mut normally_reduced, TOPIC_CONTROL, &payload, 0.0);
    assert_eq!(
        reduced.finish_interview.as_deref(),
        Some("finish-producer-marker")
    );
    assert!(normally_reduced.ended);
    assert_eq!(normally_reduced.code, "fn final_reducer_marker() {}");

    let original_code = rig.turn.state.code.clone();
    {
        let boot = crate::runtime::bootstrap(&rig.config, "recovery-test", Some("two-sum"), 45);
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
        driver.packet(
            Some(TOPIC_CONTROL),
            Some("candidate"),
            &serde_json::to_vec(&payload).unwrap(),
        );
        assert!(
            matches!(&driver.terminal, Some(Terminal::End { topic, payload: actual }) if topic == TOPIC_CONTROL && actual == &payload)
        );
        assert!(!driver.context.state.ended);
        assert_eq!(driver.context.state.code, original_code);
        assert_eq!(
            *driver.context.state, original,
            "End interception must precede every reducer effect"
        );
        assert!(driver.action.is_none());
        assert!(driver.held_contexts.is_empty());
    }
    rig.finish().await;
}

#[derive(Clone, Copy, Debug)]
enum RoomWait {
    Round,
    Thinking,
    Framework,
    Eviction,
}

impl RoomWait {
    fn message_type(self) -> Option<&'static str> {
        match self {
            Self::Round => Some("round_state"),
            Self::Thinking => Some("thinking_state"),
            Self::Framework => Some("framework_state"),
            Self::Eviction => None,
        }
    }
    fn briefing_frames(self) -> usize {
        if matches!(self, Self::Thinking) { 1 } else { 2 }
    }
    fn drain_cause(self) -> &'static str {
        if matches!(self, Self::Thinking) {
            "candidate"
        } else {
            "recovery"
        }
    }
}

struct PendingEviction {
    url: String,
    request: oneshot::Receiver<()>,
    finish: oneshot::Sender<()>,
    server: tokio::task::JoinHandle<()>,
}

async fn pending_eviction_service() -> PendingEviction {
    let (listener, endpoint) = listener().await;
    let (request, received) = oneshot::channel();
    let (finish, finished) = oneshot::channel();
    let server = tokio::spawn(async move {
        use tokio::io::AsyncReadExt;
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut header = Vec::new();
        while !header.ends_with(b"\r\n\r\n") {
            assert!(header.len() < 16 * 1024);
            header.push(socket.read_u8().await.unwrap());
        }
        let header = String::from_utf8(header).unwrap();
        assert!(
            header.starts_with("POST /twirp/livekit.RoomService/RemoveParticipant HTTP/1.1\r\n")
        );
        assert!(header.lines().any(|line| {
            line.to_ascii_lowercase()
                .starts_with("authorization: bearer ")
        }));
        let length: usize = header
            .lines()
            .find_map(|line| {
                let (key, value) = line.split_once(':')?;
                key.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse().unwrap())
            })
            .unwrap();
        assert!(length < 4096);
        let mut body = vec![0; length];
        socket.read_exact(&mut body).await.unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&body).unwrap(),
            serde_json::json!({"room":"recovery-test", "identity":"duplicate-first"})
        );
        request.send(()).unwrap();

        // Never answer. Observe cancellation only after the stimulus has
        // resumed time, so virtual Deadline advancement cannot expire this
        // peer's cleanup check before the client has been cancelled.
        finished.await.unwrap();
        tokio::time::timeout(Duration::from_secs(2), closed_tcp(socket))
            .await
            .unwrap();
        assert_eq!(
            listener.into_std().unwrap().accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    });
    PendingEviction {
        url: endpoint,
        request: received,
        finish,
        server,
    }
}

fn room_wait_usage(output: &std::process::Output, phase: RoomWait) {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let usage = stderr
        .lines()
        .filter(|line| line.starts_with("codetrial live_turn_usage "))
        .collect::<Vec<_>>();
    assert_eq!(usage.len(), 10);
    for pair in usage.as_chunks::<2>().0 {
        for (line, socket, cause, prompt, event) in [
            (pair[0], 1, "tool", 11, 1),
            (pair[1], 2, phase.drain_cause(), 29, 2),
        ] {
            for field in [
                format!("socket={socket}"),
                format!("cause={cause}"),
                format!("prompt_tokens={prompt}"),
                format!("usage_event={event}"),
            ] {
                assert!(
                    line.split_whitespace().any(|value| value == field),
                    "{line}"
                );
            }
        }
    }
    let ingress = stderr
        .lines()
        .filter(|line| line.starts_with("recovery ingress: "))
        .collect::<Vec<_>>();
    assert_eq!(ingress.len(), 5);
    for line in ingress {
        let expected = format!(
            "omitted_evictions={}",
            usize::from(matches!(phase, RoomWait::Eviction))
        );
        assert!(
            line.split_whitespace().any(|field| field == expected),
            "{line}"
        );
    }
}

async fn room_wait_terminal_matrix(phase: RoomWait) {
    let mut rows = 0;
    for cause in PREFIX_STOPS {
        let (listener, endpoint) = listener().await;
        let mut rig = Rig::with_old_usage(endpoint, Some(11)).await;
        rig.gemini.input_cause = Some("tool");
        let old = rig.gemini.inspect_reader_for_test().unwrap();
        wait_for_received_usage(&old).await;
        let mut eviction = if matches!(phase, RoomWait::Eviction) {
            let peer = pending_eviction_service().await;
            rig.config.livekit_url = peer.url.clone();
            rig.recorder
                .seed_evictions(&["duplicate-first", "duplicate-second"]);
            Some(peer)
        } else {
            None
        };
        if matches!(phase, RoomWait::Round) {
            rig.turn.state.coding_minutes = 0;
            rig.turn.state.judged_code = rig.turn.state.code.clone();
            rig.turn.state.judged_language = rig.turn.state.language.clone();
            assert!(!crate::agent::phase_judge_due(&rig.turn.state));
        }
        if matches!(phase, RoomWait::Framework) {
            crate::agent::record_framework_evidence(&mut rig.turn.state, &serde_json::json!({"phase":"repeat", "source":"candidate_speech", "kind":"observed", "confidence":80, "summary":"Authored synthetic restatement."})).unwrap();
            assert_eq!(
                session::framework_progress_unpublished(&rig.turn.state),
                Some(vec!["repeat"])
            );
        }
        let instructions =
            crate::runtime::bootstrap(&rig.config, "recovery-test", Some("two-sum"), 45)
                .instructions;
        let (body, body_received) = oneshot::channel();
        let (finish, finished) = oneshot::channel();
        let server = tokio::spawn(async move {
            use tokio_tungstenite::tungstenite::{Error as WsError, error::ProtocolError};
            let (socket, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
            let setup = socket.next().await.unwrap().unwrap();
            let setup: serde_json::Value = serde_json::from_str(setup.to_text().unwrap()).unwrap();
            assert_eq!(
                setup["setup"]["systemInstruction"]["parts"][0]["text"],
                instructions
            );
            assert!(setup["setup"]["sessionResumption"]["handle"].is_null());
            socket
                .send(Message::Text(r#"{"setupComplete":{}}"#.into()))
                .await
                .unwrap();
            socket
                .send(Message::Text(
                    r#"{"usageMetadata":{"promptTokenCount":29}}"#.into(),
                ))
                .await
                .unwrap();
            let mut messages = Vec::new();
            for _ in 0..phase.briefing_frames() {
                let frame = socket.next().await.unwrap().unwrap();
                messages.push(
                    serde_json::from_str::<serde_json::Value>(frame.to_text().unwrap()).unwrap(),
                );
            }
            body.send(()).unwrap();

            // Keep the physical peer alive until the caller has checked cleanup
            // in the return poll; no peer EOF can complete its reader.
            finished.await.unwrap();
            while let Some(frame) = socket.next().await {
                match frame {
                    Ok(frame) if frame.is_close() => break,
                    Ok(frame) => {
                        messages.push(serde_json::from_str(frame.to_text().unwrap()).unwrap())
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
                    Err(error) => panic!("unexpected fresh peer error: {error}"),
                }
            }
            assert_eq!(
                listener.into_std().unwrap().accept().unwrap_err().kind(),
                std::io::ErrorKind::WouldBlock
            );
            messages
        });
        let (setup_notice, release_setup) =
            rig.recorder.hold_data("interviewer_state", Some(false));
        let target = phase
            .message_type()
            .map(|name| rig.recorder.hold_data(name, None));
        let (reader, mut readers) = mpsc::unbounded_channel();
        rig.loops.recovery_reader = Some(reader);
        let (probe, mut observed) = mpsc::unbounded_channel();
        rig.loops.recovery_terminal = Some(probe);
        let (phase_probe, mut phases) = mpsc::unbounded_channel();
        rig.loops.recovery_phase = Some(phase_probe);
        rig.loops.deadline_at = tokio::time::Instant::now() + Duration::from_secs(8);
        let deadline = rig.loops.deadline_at;
        let presence_now = std::sync::Arc::new(std::sync::Mutex::new(Instant::now()));
        if matches!(cause, OpenStop::Absent) {
            rig.loops.presence.left(*presence_now.lock().unwrap());
            rig.loops.presence.test_now = Some(presence_now.clone());
        }
        let (unused, _) = mpsc::unbounded_channel();
        let mut room = Some(std::mem::replace(&mut rig._room_sender, unused));
        let tx = rig.tx.clone();
        let fresh_state = std::cell::RefCell::new(None::<crate::gemini::LiveReaderInspection>);
        let running = async {
            let flow = rig.run().await;
            assert!(old.is_finished());
            assert_eq!(old.pending_usage_count(), 0);
            let inspected = fresh_state.borrow();
            let fresh = inspected
                .as_ref()
                .expect("fresh receipt barrier preceded terminal");
            assert!(
                fresh.is_finished(),
                "notice cancellation joins fresh reader in return poll: {phase:?} {cause:?}"
            );
            assert_eq!(fresh.pending_usage_count(), 0);
            flow
        };
        let stimulus = async {
            setup_notice.notified().await;
            let fresh = readers.recv().await.unwrap();
            wait_for_received_usage(&fresh).await;
            assert!(!fresh.is_finished());
            assert!(old.is_finished());
            assert_eq!(old.pending_usage_count(), 0);
            fresh_state.replace(Some(fresh));
            match phase {
                RoomWait::Round => control(
                    &tx,
                    serde_json::json!({"type":"round_transition", "round":"behavioral"}),
                ),
                RoomWait::Thinking => {
                    control(&tx, serde_json::json!({"type":"thinking", "thinking":true}))
                }
                _ => {}
            }
            release_setup.notify_one();
            if let Some((entered, _)) = &target {
                entered.notified().await;
            } else {
                let peer = eviction.as_mut().unwrap();
                (&mut peer.request).await.unwrap();
                while phases.recv().await.expect("phase probe closed") != "duplicate_eviction" {}
            }
            body_received.await.unwrap();
            assert!(tokio::time::Instant::now() < deadline);
            assert!(!fresh_state.borrow().as_ref().unwrap().is_finished());
            stop_pending_open(
                cause,
                deadline,
                &tx,
                &mut room,
                &mut observed,
                &presence_now,
            )
            .await;
        };
        let (flow, ()) = tokio::join!(running, stimulus);
        assert!(flow.is_break());
        assert_eq!(rig.loops.deadline_at, deadline);
        assert_eq!(rig.loops.restarts, 1);
        assert_eq!(rig.turn.activity.live_socket, 2);
        assert_eq!(rig.turn.state.evidence_ledger.metrics.live_setup_count, 1);
        assert_eq!(rig.turn.activity.live_usage.samples, 2);
        assert_eq!(rig.turn.activity.live_usage.prompt, 40);
        assert!(!rig.turn.activity.phase_judge.is_running());
        assert!(rig.turn.activity.pending_round_transition.is_none());
        assert!(observed.try_recv().is_err());
        assert!(readers.try_recv().is_err());
        assert!(
            rig.gemini
                .send_text("no notice replacement installed")
                .await
                .is_err()
        );
        match phase {
            RoomWait::Round => {
                assert!(rig.turn.state.round_transition_seen);
                assert_eq!(rig.loops.pending_round_notice, Some("skipped"));
            }
            RoomWait::Thinking => assert!(rig.turn.state.interviewer_silenced()),
            RoomWait::Framework => assert_eq!(
                session::framework_progress_unpublished(&rig.turn.state),
                Some(vec!["repeat"])
            ),
            RoomWait::Eviction => {}
        }
        let sent = rig.recorder.take();
        for message_type in ["round_state", "thinking_state", "framework_state"] {
            assert!(!sent.iter().any(|record| matches!(record, room_link::Sent::Data { message, .. } if message["type"] == message_type)), "no pending or later notice may publish: {phase:?} {message_type}");
        }
        rig.finish().await;
        finish.send(()).unwrap();
        let messages = server.await.unwrap();
        assert_eq!(messages.len(), phase.briefing_frames());
        assert_eq!(messages[0]["clientContent"]["turnComplete"], false);
        if phase.briefing_frames() == 2 {
            assert_eq!(messages[1]["clientContent"]["turnComplete"], true);
        }
        if let Some(peer) = eviction {
            peer.finish.send(()).unwrap();
            peer.server.await.unwrap();
        }
        rows += 1;
    }
    assert_eq!(rows, 5);
}

#[tokio::test(flavor = "current_thread")]
async fn room_round_notice_remains_cancellable_with_received_evidence() {
    if isolated_checked(
        "room_round_notice_remains_cancellable_with_received_evidence",
        |out| room_wait_usage(out, RoomWait::Round),
    ) {
        return;
    }
    room_wait_terminal_matrix(RoomWait::Round).await;
}
#[tokio::test(flavor = "current_thread")]
async fn room_thinking_notice_remains_cancellable_with_received_evidence() {
    if isolated_checked(
        "room_thinking_notice_remains_cancellable_with_received_evidence",
        |out| room_wait_usage(out, RoomWait::Thinking),
    ) {
        return;
    }
    room_wait_terminal_matrix(RoomWait::Thinking).await;
}
#[tokio::test(flavor = "current_thread")]
async fn room_framework_notice_remains_cancellable_with_received_evidence() {
    if isolated_checked(
        "room_framework_notice_remains_cancellable_with_received_evidence",
        |out| room_wait_usage(out, RoomWait::Framework),
    ) {
        return;
    }
    room_wait_terminal_matrix(RoomWait::Framework).await;
}
#[tokio::test(flavor = "current_thread")]
async fn room_duplicate_eviction_remains_cancellable_with_received_evidence() {
    if isolated_checked_with_loopback(
        "room_duplicate_eviction_remains_cancellable_with_received_evidence",
        true,
        |out| room_wait_usage(out, RoomWait::Eviction),
    ) {
        return;
    }
    room_wait_terminal_matrix(RoomWait::Eviction).await;
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum WriteSite {
    Brief,
    HeldContext,
    HeldAction,
    BoardImage,
    ListeningBrief,
    SilentEditor,
}

impl WriteSite {
    fn label(self) -> &'static str {
        match self {
            Self::Brief => "recovery_brief",
            Self::HeldContext => "held_context",
            Self::HeldAction => "held_action",
            Self::BoardImage => "board_image",
            Self::ListeningBrief => "listening_brief",
            Self::SilentEditor => "silent_editor",
        }
    }

    // Frames the write itself puts on the wire.
    fn frames(self) -> usize {
        match self {
            Self::Brief | Self::HeldAction => 2,
            _ => 1,
        }
    }

    // The cause of the fresh socket's received usage once the write itself has
    // completed: a reply-bearing write is attributed to recovery, a silent
    // context write to the candidate.
    fn after_cause(self) -> &'static str {
        match self {
            Self::Brief | Self::HeldAction | Self::BoardImage => "recovery",
            Self::HeldContext | Self::ListeningBrief | Self::SilentEditor => "candidate",
        }
    }

    // Frames the fresh peer must have received before the targeted write: the
    // briefing (one silent frame while Thinking is held, otherwise a context
    // and a reply frame), plus the held context for an action.
    fn prior_frames(self) -> usize {
        match self {
            Self::Brief | Self::ListeningBrief => 0,
            Self::HeldContext | Self::SilentEditor => 1,
            Self::BoardImage | Self::HeldAction => 2,
        }
    }
    fn drain_cause(self) -> &'static str {
        match self {
            Self::BoardImage => "recovery",
            Self::Brief
            | Self::HeldContext
            | Self::HeldAction
            | Self::ListeningBrief
            | Self::SilentEditor => "candidate",
        }
    }
}

fn write_site_usage(output: &std::process::Output, drain: &str, rows: usize) {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let usage = stderr
        .lines()
        .filter(|line| line.starts_with("codetrial live_turn_usage "))
        .collect::<Vec<_>>();
    assert_eq!(usage.len(), rows * 2);
    for pair in usage.as_chunks::<2>().0 {
        for (line, socket, cause, prompt, event) in
            [(pair[0], 1, "tool", 11, 1), (pair[1], 2, drain, 29, 2)]
        {
            for field in [
                format!("socket={socket}"),
                format!("cause={cause}"),
                format!("prompt_tokens={prompt}"),
                format!("usage_event={event}"),
            ] {
                assert!(
                    line.split_whitespace().any(|value| value == field),
                    "{line}"
                );
            }
        }
    }
    assert_eq!(
        stderr
            .lines()
            .filter(|line| line.starts_with("recovery ingress: "))
            .count(),
        rows
    );
}

// `physical` replaces the before-write gate with a peer whose receive window is
// tiny and which stops reading, so the targeted write really pends on the
// socket. Only End is exercised there.
//
// `after` instead completes the write and queues the terminal in that same
// poll, so the next boundary (not a gate) must observe it. Deadline is not in
// that matrix: it is a clock test, not a queued input.
async fn write_site_terminal_matrix(site: WriteSite, physical: bool, after: bool) {
    let causes: &[OpenStop] = if physical {
        &[OpenStop::End]
    } else if after {
        &[
            OpenStop::End,
            OpenStop::RoomDisconnected,
            OpenStop::EventsEof,
            OpenStop::Absent,
        ]
    } else {
        &PREFIX_STOPS
    };
    let mut rows = 0;
    for &cause in causes {
        let resumed = site == WriteSite::SilentEditor;
        let mut rig = Rig::with_old_checkpoint("ws://127.0.0.1:9/".into(), Some(11), resumed).await;
        rig.gemini.input_cause = Some("tool");
        let old = rig.gemini.inspect_reader_for_test().unwrap();
        wait_for_received_usage(&old).await;
        if matches!(site, WriteSite::ListeningBrief | WriteSite::SilentEditor) {
            // Speech handoff without a prefix finalization. A resumed socket
            // (the editor site) also needs a checkpoint newer than the handoff.
            rig.speech = Some(session::SpeechHandoff {
                checkpoint_after: Instant::now()
                    .checked_sub(Duration::from_secs(30))
                    .unwrap_or_else(Instant::now),
                transcript_fragments: vec![PREFIX_TEXT.into()],
                discarded: 0,
            });
        }
        if resumed {
            rig.turn.state.code = "def solve(): return 7".into();
            rig.turn.state.language = "python".into();
        }
        if matches!(site, WriteSite::HeldContext | WriteSite::HeldAction) {
            rig.turn.state.thinking_hold = crate::agent::ThinkingHold::Held {
                since: Instant::now(),
            };
            rig.turn.state.code = "def solve(): return 7".into();
            rig.turn.state.language = "python".into();
            rig.turn.activity.last_test_reaction = Instant::now() - Duration::from_secs(100);
        }
        if matches!(site, WriteSite::BoardImage) {
            rig.turn.state.interview_mode = crate::agent::InterviewMode::Whiteboard;
            rig.board.seed_recovery_image(if physical {
                vec![0x5a; 24 * 1024 * 1024]
            } else {
                vec![0x5a; 64]
            });
        }
        let listener = tokio::net::TcpSocket::new_v4().unwrap();
        if physical {
            listener.set_recv_buffer_size(1024).unwrap();
        }
        listener.bind("127.0.0.1:0".parse().unwrap()).unwrap();
        let listener = listener.listen(1).unwrap();
        rig.endpoint = format!("ws://{}/", listener.local_addr().unwrap());
        let instructions =
            crate::runtime::bootstrap(&rig.config, "recovery-test", Some("two-sum"), 45)
                .instructions;
        let expected_before = site.prior_frames() + if after { site.frames() } else { 0 };
        let (body, body_received) = oneshot::channel();
        let (finish, finished) = oneshot::channel();
        let server = tokio::spawn(async move {
            use tokio_tungstenite::tungstenite::{Error as WsError, error::ProtocolError};
            let (socket, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
            let setup = socket.next().await.unwrap().unwrap();
            let setup: serde_json::Value = serde_json::from_str(setup.to_text().unwrap()).unwrap();
            assert_eq!(
                setup["setup"]["systemInstruction"]["parts"][0]["text"],
                instructions
            );
            assert_eq!(
                setup["setup"]["sessionResumption"]["handle"],
                if resumed {
                    serde_json::json!("matrix-checkpoint")
                } else {
                    serde_json::Value::Null
                }
            );
            socket
                .send(Message::Text(r#"{"setupComplete":{}}"#.into()))
                .await
                .unwrap();
            socket
                .send(Message::Text(
                    r#"{"usageMetadata":{"promptTokenCount":29}}"#.into(),
                ))
                .await
                .unwrap();
            let mut messages = Vec::new();
            for _ in 0..expected_before {
                let frame = socket.next().await.unwrap().unwrap();
                messages.push(
                    serde_json::from_str::<serde_json::Value>(frame.to_text().unwrap()).unwrap(),
                );
            }
            body.send(()).unwrap();
            finished.await.unwrap();
            if physical {
                // Never read the stalled write; observe only its release.
                let mut socket = socket.into_inner();
                use tokio::io::AsyncReadExt;
                let mut bytes = [0; 65536];
                let mut received = 0;
                loop {
                    match socket.read(&mut bytes).await {
                        Ok(0) => break,
                        Ok(count) => received += count,
                        Err(error) if error.kind() == std::io::ErrorKind::ConnectionReset => break,
                        other => panic!("cancelled write must release the transport: {other:?}"),
                    }
                }
                return (messages, Some(received));
            }
            while let Some(frame) = socket.next().await {
                match frame {
                    Ok(frame) if frame.is_close() => break,
                    Ok(frame) => {
                        messages.push(serde_json::from_str(frame.to_text().unwrap()).unwrap())
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
                    Err(error) => panic!("unexpected fresh peer error: {error}"),
                }
            }
            (messages, None)
        });
        let (setup_notice, release_setup) =
            rig.recorder.hold_data("interviewer_state", Some(false));
        let gate = (!physical && !after).then(|| {
            let entered = std::sync::Arc::new(tokio::sync::Notify::new());
            rig.loops.recovery_writes.push_back(WriteBarrier {
                phase: site.label(),
                entered: entered.clone(),
                release: std::sync::Arc::new(tokio::sync::Notify::new()),
            });
            entered
        });

        // Releasing Thinking while the held context write is gated produces the
        // action after the briefing, so it is the next fresh write.
        let context_gate = (site == WriteSite::HeldAction).then(|| {
            let entered = std::sync::Arc::new(tokio::sync::Notify::new());
            let release = std::sync::Arc::new(tokio::sync::Notify::new());
            rig.loops.recovery_writes.push_front(WriteBarrier {
                phase: "held_context",
                entered: entered.clone(),
                release: release.clone(),
            });
            (entered, release)
        });
        let (reader, mut readers) = mpsc::unbounded_channel();
        rig.loops.recovery_reader = Some(reader);
        let (probe, mut observed) = mpsc::unbounded_channel();
        rig.loops.recovery_terminal = Some(probe);
        let (phase_probe, mut phases) = mpsc::unbounded_channel();
        rig.loops.recovery_phase = Some(phase_probe);
        rig.loops.deadline_at = tokio::time::Instant::now() + Duration::from_secs(8);
        let deadline = rig.loops.deadline_at;
        let presence_now = std::sync::Arc::new(std::sync::Mutex::new(Instant::now()));
        if matches!(cause, OpenStop::Absent) {
            rig.loops.presence.left(*presence_now.lock().unwrap());
            rig.loops.presence.test_now = Some(presence_now.clone());
        }
        let (unused, _) = mpsc::unbounded_channel();
        let mut room = Some(std::mem::replace(&mut rig._room_sender, unused));
        let tx = rig.tx.clone();
        if after {
            let tx = tx.clone();
            // Only the cause under test may touch the room-event channel.
            let sender = match cause {
                OpenStop::EventsEof => room.take(),
                OpenStop::RoomDisconnected => room.clone(),
                _ => None,
            };
            let presence_now = presence_now.clone();
            rig.loops.recovery_after.push_back(AfterWrite {
                phase: site.label(),
                action: Box::new(move || match cause {
                    OpenStop::End => control(&tx, serde_json::json!({"type":"end_interview"})),
                    OpenStop::RoomDisconnected => sender
                        .unwrap()
                        .send(RoomEvent::Disconnected {
                            reason: DisconnectReason::ClientInitiated,
                        })
                        .unwrap(),
                    OpenStop::EventsEof => drop(sender),
                    OpenStop::Absent => {
                        *presence_now.lock().unwrap() += CANDIDATE_ABSENCE_LIMIT;
                        tx.send(Packet {
                            topic: Some(TOPIC_CONTROL.into()),
                            sender: Some("bystander".into()),
                            payload: b"{}".to_vec(),
                        })
                        .unwrap();
                    }
                    OpenStop::Deadline => unreachable!("not a queued input"),
                }),
            });
        }
        let fresh_state = std::cell::RefCell::new(None::<crate::gemini::LiveReaderInspection>);
        let running = async {
            let flow = rig.run().await;
            assert!(old.is_finished());
            assert_eq!(old.pending_usage_count(), 0);
            let inspected = fresh_state.borrow();
            let fresh = inspected
                .as_ref()
                .expect("fresh receipt barrier preceded terminal");
            assert!(
                fresh.is_finished(),
                "write cancellation joins fresh reader in return poll: {site:?} {cause:?}"
            );
            assert_eq!(fresh.pending_usage_count(), 0);
            flow
        };
        let stimulus = async {
            setup_notice.notified().await;
            let fresh = readers.recv().await.unwrap();
            wait_for_received_usage(&fresh).await;
            assert!(!fresh.is_finished());
            assert!(old.is_finished());
            fresh_state.replace(Some(fresh));
            if matches!(site, WriteSite::HeldContext | WriteSite::HeldAction) {
                tx.send(Packet {
                    topic: Some(crate::runtime::TOPIC_TEST_RESULTS.into()),
                    sender: Some("candidate".into()),
                    payload: serde_json::to_vec(&serde_json::json!({
                        "passed": 0, "total": 1, "code":"def solve(): return 7",
                        "language":"python"
                    }))
                    .unwrap(),
                })
                .unwrap();
            }
            release_setup.notify_one();
            if let Some((entered, release)) = &context_gate {
                entered.notified().await;
                control(
                    &tx,
                    serde_json::json!({"type":"thinking", "thinking":false}),
                );
                release.notify_one();
            }
            if after {
                // The completed write queues the terminal itself.
            } else if let Some(entered) = &gate {
                entered.notified().await;
            } else {
                while phases.recv().await.expect("phase probe closed") != site.label() {}
                // A physically blocked write cannot finish on its own.
                tokio::time::sleep(Duration::from_millis(300)).await;
                assert!(phases.try_recv().is_err());
                assert!(observed.try_recv().is_err());
                assert!(!fresh_state.borrow().as_ref().unwrap().is_finished());
            }
            body_received.await.unwrap();
            assert!(tokio::time::Instant::now() < deadline);
            if !after {
                assert!(!fresh_state.borrow().as_ref().unwrap().is_finished());
                stop_pending_open(
                    cause,
                    deadline,
                    &tx,
                    &mut room,
                    &mut observed,
                    &presence_now,
                )
                .await;
            }
        };
        let (flow, ()) = tokio::join!(running, stimulus);
        assert!(flow.is_break());
        assert_eq!(rig.loops.deadline_at, deadline);
        assert_eq!(rig.loops.restarts, 1);
        assert_eq!(rig.turn.activity.live_socket, 2);
        assert_eq!(rig.turn.state.evidence_ledger.metrics.live_setup_count, 1);
        assert_eq!(rig.turn.activity.live_usage.samples, 2);
        assert_eq!(rig.turn.activity.live_usage.prompt, 40);
        if after {
            let terminal = observed.try_recv().expect("the queued terminal was taken");
            let shown = match &terminal {
                Terminal::End { .. } => "End",
                Terminal::Finish(_) => "Finish",
                Terminal::Deadline => "Deadline",
                Terminal::Absent => "Absent",
                Terminal::RoomClosed => "RoomClosed",
            };
            assert!(
                matches!(
                    (cause, terminal),
                    (OpenStop::End, Terminal::End { .. })
                        | (OpenStop::Absent, Terminal::Absent)
                        | (
                            OpenStop::RoomDisconnected | OpenStop::EventsEof,
                            Terminal::RoomClosed
                        )
                ),
                "{site:?} {cause:?} {shown}"
            );
        }
        if matches!(site, WriteSite::HeldContext | WriteSite::HeldAction) {
            // The held text may be lost with the discarded socket, but the fact
            // it reported is state the ending owner reads.
            assert_eq!(rig.turn.state.test_runs, 1, "{site:?} {cause:?}");
        }
        assert!(observed.try_recv().is_err());
        assert!(readers.try_recv().is_err());
        assert!(
            rig.gemini
                .send_text("no replacement installed")
                .await
                .is_err()
        );
        assert!(rig.loops.recovery_writes.is_empty());
        rig.finish().await;
        finish.send(()).unwrap();
        let (messages, received) = server.await.unwrap();
        // The unread image was still in flight: the peer never held all of it.
        assert_eq!(received.is_some(), physical);
        assert!(received.is_none_or(|bytes| bytes < 24 * 1024 * 1024));
        assert_eq!(
            messages.len(),
            expected_before,
            "no write after the targeted one may reach the wire: {site:?} {cause:?}"
        );
        rows += 1;
    }
    assert_eq!(rows, causes.len());
}

macro_rules! write_site_test {
    ($name:ident, $site:expr, $physical:expr) => {
        write_site_test!($name, $site, $physical, false);
    };
    ($name:ident, $site:expr, $physical:expr, $after:expr) => {
        #[tokio::test(flavor = "current_thread")]
        async fn $name() {
            let rows = if $physical {
                1
            } else if $after {
                4
            } else {
                5
            };
            let drain = if $after {
                $site.after_cause()
            } else {
                $site.drain_cause()
            };
            if isolated_checked(stringify!($name), |out| write_site_usage(out, drain, rows)) {
                return;
            }
            write_site_terminal_matrix($site, $physical, $after).await;
        }
    };
}

write_site_test!(brief_write_cancels_before_the_wire, WriteSite::Brief, false);
write_site_test!(
    held_context_write_cancels_before_the_wire,
    WriteSite::HeldContext,
    false
);
write_site_test!(
    held_action_write_cancels_before_the_wire,
    WriteSite::HeldAction,
    false
);
write_site_test!(
    board_image_write_cancels_before_the_wire,
    WriteSite::BoardImage,
    false
);
write_site_test!(
    board_image_write_cancels_under_backpressure,
    WriteSite::BoardImage,
    true
);
write_site_test!(
    listening_brief_write_cancels_before_the_wire,
    WriteSite::ListeningBrief,
    false
);
write_site_test!(
    silent_editor_write_cancels_before_the_wire,
    WriteSite::SilentEditor,
    false
);
write_site_test!(
    brief_completion_yields_to_a_queued_terminal,
    WriteSite::Brief,
    false,
    true
);
write_site_test!(
    held_context_completion_yields_to_a_queued_terminal,
    WriteSite::HeldContext,
    false,
    true
);
write_site_test!(
    held_action_completion_yields_to_a_queued_terminal,
    WriteSite::HeldAction,
    false,
    true
);
write_site_test!(
    board_image_completion_yields_to_a_queued_terminal,
    WriteSite::BoardImage,
    false,
    true
);
write_site_test!(
    listening_brief_completion_yields_to_a_queued_terminal,
    WriteSite::ListeningBrief,
    false,
    true
);
write_site_test!(
    silent_editor_completion_yields_to_a_queued_terminal,
    WriteSite::SilentEditor,
    false,
    true
);

#[tokio::test]
async fn a_held_action_settles_only_the_revision_it_carried() {
    if isolated("a_held_action_settles_only_the_revision_it_carried") {
        return;
    }
    let (listener, endpoint) = listener().await;
    let mut rig = Rig::new(endpoint).await;
    rig.turn.state.thinking_hold = crate::agent::ThinkingHold::Held {
        since: Instant::now(),
    };
    rig.turn.state.code = "def solve(): return 7".into();
    rig.turn.state.language = "python".into();
    rig.turn.activity.last_test_reaction = Instant::now() - Duration::from_secs(100);
    let mut gates = Vec::new();
    for phase in ["held_context", "held_action"] {
        let entered = std::sync::Arc::new(tokio::sync::Notify::new());
        let release = std::sync::Arc::new(tokio::sync::Notify::new());
        rig.loops.recovery_writes.push_back(WriteBarrier {
            phase,
            entered: entered.clone(),
            release: release.clone(),
        });
        gates.push((entered, release));
    }
    let tx = rig.tx.clone();
    tx.send(Packet {
        topic: Some(crate::runtime::TOPIC_TEST_RESULTS.into()),
        sender: Some("candidate".into()),
        payload: serde_json::to_vec(&serde_json::json!({
            "passed": 0, "total": 1, "code":"def solve(): return 7", "language":"python"
        }))
        .unwrap(),
    })
    .unwrap();
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
        socket.next().await.unwrap().unwrap();
        socket
            .send(Message::Text(r#"{"setupComplete":{}}"#.into()))
            .await
            .unwrap();
        let mut messages = Vec::new();
        while let Some(Ok(frame)) = socket.next().await {
            if frame.is_close() {
                break;
            }
            messages
                .push(serde_json::from_str::<serde_json::Value>(frame.to_text().unwrap()).unwrap());
        }
        messages
    });
    let stimulus = async {
        gates[0].0.notified().await;
        control(
            &tx,
            serde_json::json!({"type":"thinking", "thinking":false}),
        );
        gates[0].1.notify_one();
        gates[1].0.notified().await;
        // A newer hold arrives while the older action is on the wire.
        control(&tx, serde_json::json!({"type":"thinking", "thinking":true}));
        gates[1].1.notify_one();
    };
    let (flow, ()) = tokio::join!(rig.run(), stimulus);
    assert!(flow.is_continue());
    assert!(rig.turn.state.interviewer_silenced());
    assert!(
        rig.turn.state.owed_reply_on_resume.is_some(),
        "an older action's delivery cannot settle the newer hold's debt"
    );
    assert!(rig.loops.recovery_writes.is_empty());
    rig.finish().await;
    let messages = server.await.unwrap();
    assert!(
        messages.len() >= 3
            && messages
                .iter()
                .any(|message| message.to_string().contains("ready after thinking time")),
        "briefing, context and the older action each reach the wire"
    );
}

enum FreshStep {
    Ready,
    DropAfterSetup,
    CloseAfterSetup(u16, &'static str),
    Refuse(axum::http::StatusCode),
}

struct Accepted {
    key: String,
    handle: Option<serde_json::Value>,
}

// Plays one scripted step per accepted connection and records what the client
// asked for. Connections beyond the script are never accepted. The handshake
// callback's error type is a full HTTP response.
#[allow(clippy::result_large_err)]
async fn scripted_fresh_gemini(
    listener: tokio::net::TcpListener,
    script: Vec<FreshStep>,
) -> (Vec<Accepted>, tokio::net::TcpListener) {
    use tokio_tungstenite::tungstenite::handshake::server;
    let mut seen = Vec::new();
    for step in script {
        let (socket, _) = listener.accept().await.unwrap();
        let mut key = String::new();
        let refuse = match &step {
            FreshStep::Refuse(status) => Some(*status),
            _ => None,
        };
        let accepted = tokio_tungstenite::accept_hdr_async(
            socket,
            |request: &server::Request, response: server::Response| {
                key = request
                    .uri()
                    .query()
                    .unwrap_or_default()
                    .split('&')
                    .find_map(|pair| pair.strip_prefix("key=").map(str::to_owned))
                    .unwrap_or_default();
                if let Some(status) = refuse {
                    let mut refusal = server::ErrorResponse::new(None);
                    *refusal.status_mut() = status;
                    Err(refusal)
                } else {
                    Ok(response)
                }
            },
        )
        .await;
        let Ok(mut socket) = accepted else {
            seen.push(Accepted { key, handle: None });
            continue;
        };
        let setup = socket.next().await.unwrap().unwrap();
        let setup: serde_json::Value = serde_json::from_str(setup.to_text().unwrap()).unwrap();
        seen.push(Accepted {
            key,
            handle: Some(setup["setup"]["sessionResumption"]["handle"].clone())
                .filter(|handle| !handle.is_null()),
        });
        match step {
            FreshStep::DropAfterSetup => drop(socket),
            FreshStep::CloseAfterSetup(code, reason) => {
                socket
                    .send(Message::Close(Some(
                        tokio_tungstenite::tungstenite::protocol::CloseFrame {
                            code: code.into(),
                            reason: reason.into(),
                        },
                    )))
                    .await
                    .unwrap();
            }
            FreshStep::Ready => {
                socket
                    .send(Message::Text(r#"{"setupComplete":{}}"#.into()))
                    .await
                    .unwrap();
                while let Some(Ok(frame)) = socket.next().await {
                    if frame.is_close() {
                        break;
                    }
                }
            }
            FreshStep::Refuse(_) => unreachable!("refused during the handshake"),
        }
    }
    (seen, listener)
}

#[derive(Debug, PartialEq)]
enum Replaced {
    Ready,
    Billing,
    Unreachable,
}

struct ReplacementCase {
    name: &'static str,
    update: Option<serde_json::Value>,
    keys: &'static str,
    stale_checkpoint: bool,
    young_resumed: bool,
    restarts: usize,
    script: Vec<FreshStep>,
    // (key, resume handle offered) per accepted connection, in order.
    accepts: Vec<(&'static str, Option<&'static str>)>,
    fallback: Option<&'static str>,
    outcome: Replaced,
    restarts_after: usize,
    // Real time the recovery must spend waiting on a cooling key.
    min_wait: Duration,
}

fn replacement_cases() -> Vec<ReplacementCase> {
    let checkpoint = || Some(serde_json::json!({"resumable": true, "newHandle": "table-handle"}));
    let base = |name| ReplacementCase {
        name,
        update: checkpoint(),
        keys: "sequence-test-key",
        stale_checkpoint: false,
        young_resumed: false,
        restarts: 0,
        script: vec![FreshStep::Ready],
        accepts: vec![("sequence-test-key", Some("table-handle"))],
        fallback: Some("none"),
        outcome: Replaced::Ready,
        restarts_after: 1,
        min_wait: Duration::ZERO,
    };
    vec![
        base("resume_once_on_the_original_key"),
        ReplacementCase {
            update: None,
            accepts: vec![("sequence-test-key", None)],
            fallback: Some("no_handle"),
            ..base("absent_handle_opens_cold")
        },
        ReplacementCase {
            update: Some(serde_json::json!({"resumable": false, "newHandle": "unusable"})),
            accepts: vec![("sequence-test-key", None)],
            fallback: Some("no_handle"),
            ..base("handle_without_resumable_opens_cold")
        },
        ReplacementCase {
            script: vec![FreshStep::DropAfterSetup, FreshStep::Ready],
            accepts: vec![
                ("sequence-test-key", Some("table-handle")),
                ("sequence-test-key", None),
            ],
            fallback: Some("resume_refused"),
            restarts_after: 1,
            ..base("a_dropped_resume_falls_back_cold_once")
        },
        ReplacementCase {
            script: vec![
                FreshStep::CloseAfterSetup(1008, "session handle has expired"),
                FreshStep::Ready,
            ],
            accepts: vec![
                ("sequence-test-key", Some("table-handle")),
                ("sequence-test-key", None),
            ],
            fallback: Some("resume_refused"),
            ..base("a_close_before_setup_complete_is_a_refused_resume_not_retried")
        },
        ReplacementCase {
            stale_checkpoint: true,
            accepts: vec![("sequence-test-key", None)],
            fallback: Some("checkpoint_before_handoff"),
            ..base("a_checkpoint_older_than_the_handoff_is_not_offered")
        },
        ReplacementCase {
            young_resumed: true,
            accepts: vec![("sequence-test-key", None)],
            fallback: Some("resume_suppressed"),
            ..base("a_young_resumed_socket_suppresses_the_next_resume")
        },
        ReplacementCase {
            keys: "other-key,sequence-test-key",
            accepts: vec![("other-key", None)],
            fallback: Some("no_handle"),
            ..base("a_handle_never_crosses_to_another_key")
        },
        ReplacementCase {
            update: None,
            script: vec![FreshStep::Refuse(axum::http::StatusCode::PAYMENT_REQUIRED)],
            accepts: vec![("sequence-test-key", None)],
            fallback: None,
            outcome: Replaced::Billing,
            ..base("a_sole_key_billing_failure_is_final")
        },
        // A sole key has nothing to rotate to, so the retry waits out the
        // backoff and asks the same key again.
        ReplacementCase {
            update: None,
            script: vec![
                FreshStep::Refuse(axum::http::StatusCode::TOO_MANY_REQUESTS),
                FreshStep::Ready,
            ],
            accepts: vec![("sequence-test-key", None), ("sequence-test-key", None)],
            fallback: Some("no_handle"),
            restarts_after: 2,
            min_wait: crate::livekit::COLD_OPEN_BACKOFF,
            ..base("a_sole_key_rate_limit_waits_before_the_same_key")
        },
        ReplacementCase {
            update: None,
            // Distinct names: cooldowns are process-global and outlive a row.
            keys: "cooling-key,backup-key",
            script: vec![
                FreshStep::Refuse(axum::http::StatusCode::TOO_MANY_REQUESTS),
                FreshStep::Ready,
            ],
            accepts: vec![("cooling-key", None), ("backup-key", None)],
            fallback: Some("no_handle"),
            restarts_after: 2,
            min_wait: crate::livekit::COLD_OPEN_BACKOFF,
            // The retry still waits the backoff before moving to the backup.
            ..base("a_rate_limited_key_gives_way_to_the_backup_after_the_backoff")
        },
        // A refused resume still falls back cold, so billing costs one more
        // handshake here than on a cold open; both end the interview.
        ReplacementCase {
            script: vec![
                FreshStep::Refuse(axum::http::StatusCode::PAYMENT_REQUIRED),
                FreshStep::Refuse(axum::http::StatusCode::PAYMENT_REQUIRED),
            ],
            accepts: vec![("sequence-test-key", None), ("sequence-test-key", None)],
            fallback: None,
            outcome: Replaced::Billing,
            ..base("billing_on_a_resume_attempt_ends_after_one_cold_try")
        },
        ReplacementCase {
            restarts: crate::livekit::GEMINI_RESTART_LIMIT,
            script: Vec::new(),
            accepts: Vec::new(),
            fallback: None,
            outcome: Replaced::Unreachable,
            restarts_after: crate::livekit::GEMINI_RESTART_LIMIT,
            ..base("spent_charges_refuse_before_any_connection")
        },
    ]
}

#[tokio::test(flavor = "current_thread")]
async fn replacement_classification_follows_the_declared_table() {
    if isolated_checked(
        "replacement_classification_follows_the_declared_table",
        |output| {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let observed = stderr
                .lines()
                .filter(|line| line.starts_with("replacement: "))
                .filter_map(|line| {
                    line.split_whitespace()
                        .find_map(|field| field.strip_prefix("fallback="))
                })
                .collect::<Vec<_>>();
            let expected = replacement_cases()
                .iter()
                .filter_map(|case| case.fallback)
                .collect::<Vec<_>>();
            assert_eq!(observed, expected);
        },
    ) {
        return;
    }
    let cases = replacement_cases();
    assert_eq!(cases.len(), 13);
    for case in cases {
        eprintln!("replacement case: {}", case.name);
        let mut rig =
            Rig::with_old_resumption("ws://127.0.0.1:9/".into(), Some(11), case.update.clone())
                .await;
        let config = load_from_pairs([
            ("LIVEKIT_URL", "wss://example.livekit.cloud"),
            ("LIVEKIT_API_KEY", "devkey"),
            ("LIVEKIT_API_SECRET", "devsecret"),
            ("GOOGLE_API_KEYS", case.keys),
        ])
        .unwrap();
        rig.keys = Arc::new(GeminiKeys::from_config(&config));
        rig.config = config;
        let (fresh, endpoint) = listener().await;
        rig.endpoint = endpoint;
        rig.gemini.input_cause = Some("tool");
        let old = rig.gemini.inspect_reader_for_test().unwrap();
        wait_for_received_usage(&old).await;
        if case.stale_checkpoint {
            tokio::time::sleep(Duration::from_millis(20)).await;
            rig.speech = Some(session::SpeechHandoff {
                checkpoint_after: Instant::now(),
                transcript_fragments: vec![PREFIX_TEXT.into()],
                discarded: 0,
            });
        }
        if case.young_resumed {
            rig.loops.resume_recovery.connected(true);
        }
        rig.loops.restarts = case.restarts;
        if case.outcome != Replaced::Ready {
            rig.turn.state.ended = true;
        }
        let server = tokio::spawn(scripted_fresh_gemini(fresh, case.script));
        let started = Instant::now();
        let flow = rig.run().await;
        assert!(started.elapsed() >= case.min_wait, "{}", case.name);
        match case.outcome {
            Replaced::Ready => {
                assert!(flow.is_continue(), "{}", case.name);
                assert_eq!(rig.turn.activity.live_socket, 2, "{}", case.name);
            }
            Replaced::Billing => {
                assert!(flow.is_break(), "{}", case.name);
                assert_eq!(rig.turn.activity.live_exit, LiveOutcome::Billing);
            }
            Replaced::Unreachable => {
                assert!(flow.is_break(), "{}", case.name);
                assert_eq!(rig.turn.activity.live_exit, LiveOutcome::GeminiUnreachable);
            }
        }
        assert_eq!(rig.loops.restarts, case.restarts_after, "{}", case.name);
        rig.finish().await;
        let (seen, listener) = server.await.unwrap();
        let seen = seen
            .iter()
            .map(|accepted| {
                (
                    accepted.key.as_str(),
                    accepted.handle.as_ref().and_then(|handle| handle.as_str()),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(seen, case.accepts, "{}", case.name);
        // No attempt beyond the script may be waiting to connect.
        assert_eq!(
            listener.into_std().unwrap().accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock,
            "{}",
            case.name
        );
    }
}

// Characterization of the age-based restart reset (no policy change).
//
// `take_restart_attempt` clears the consecutive-charge count for any replaced
// socket whose budget age reaches `HEALTHY_GEMINI_SOCKET`, with or without
// output, as long as the socket owed no reply (`replaced_socket_age`). So a
// provider that closes an idle socket 61 seconds after opening it, forever,
// never exhausts the budget, while the same pattern at 59 seconds or with an
// unfinished reply ends the interview on the ninth close. This block pins that
// behaviour in logical time (no sleeping) and asserts no progress requirement,
// because none exists. Whether it should is a separate policy decision.
//
// The step below composes the same calls the replacement site makes:
// `replaced_socket_age`, `take_restart_attempt`, and `ResumeRecovery` fed the
// raw socket age. The replacement
// site's `trigger == Watchdog || owed` flag is held false here; an owed reply
// reaches the same zeroed age through `reply_unfinished`.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SocketEnd {
    /// The briefing's answer, then a real reply that completes.
    RealOutput,
    /// Nothing happens on the socket.
    Idle,
    /// Only the answer to the recovery briefing completes. The first socket
    /// has no briefing, so its one completion is a real reply; the charge is
    /// already zero there, so nothing observable differs.
    BriefingOnly,
    /// A candidate turn is awaiting a reply when the socket is replaced.
    OwedReply,
}

#[derive(Debug, PartialEq, Eq)]
struct BudgetRun {
    opens: usize,
    refused_at_logical_s: Option<u64>,
    max_charge: usize,
    final_charge: usize,
    age_resets: usize,
    output_resets: usize,
    resumes_allowed: usize,
}

fn run_restart_budget(sockets: usize, life_s: u64, end: SocketEnd) -> BudgetRun {
    let life = Duration::from_secs(life_s);
    let mut run = BudgetRun {
        opens: 0,
        refused_at_logical_s: None,
        max_charge: 0,
        final_charge: 0,
        age_resets: 0,
        output_resets: 0,
        resumes_allowed: 0,
    };
    let mut restarts = 0;
    let mut recovery = ResumeRecovery::default();
    let mut state = RuntimeState::default();
    for socket in 1..=sockets {
        // Every replacement is followed by a briefing; the first socket has
        // none.
        state.recovery_reply_pending = socket > 1;
        let completions = match end {
            SocketEnd::RealOutput => 2,
            SocketEnd::BriefingOnly => 1,
            SocketEnd::Idle | SocketEnd::OwedReply => 0,
        };
        for _ in 0..completions {
            let before = restarts;
            let mut generating = RuntimeActivity::new(Instant::now());
            generating.note_output(Instant::now());
            recovery.note_event(
                &mut restarts,
                &mut state,
                &generating,
                &GeminiEvent::TurnComplete,
            );
            if before > 0 && restarts == 0 {
                run.output_resets += 1;
            }
        }
        let mut activity = RuntimeActivity::new(Instant::now());
        if end == SocketEnd::OwedReply {
            activity.note_candidate_finished(Instant::now(), false);
            assert!(activity.reply_unfinished());
        } else {
            assert!(!activity.reply_unfinished());
        }
        let closed_at = socket as u64 * life_s;
        let before = restarts;
        let age = replaced_socket_age(life, false, &activity);
        if !take_restart_attempt(&mut restarts, age) {
            run.refused_at_logical_s = Some(closed_at);
            break;
        }
        if before > 0 && restarts == 1 {
            run.age_resets += 1;
        }
        run.opens += 1;
        run.max_charge = run.max_charge.max(restarts);
        let resumed = recovery.allow_resume(life);
        run.resumes_allowed += usize::from(resumed);
        recovery.connected(resumed);
    }
    run.final_charge = restarts;
    run
}

#[test]
fn age_based_restart_reset_follows_the_declared_table() {
    use SocketEnd::*;
    assert_eq!(
        GEMINI_RESTART_LIMIT, 8,
        "the table below is written for a limit of 8"
    );
    assert_eq!(HEALTHY_GEMINI_SOCKET, Duration::from_secs(60));

    // Declared before the first run. 59 sockets of `life` seconds each fit in
    // one logical hour (59 * 61 = 3599). Columns: opens, refusal time, peak and
    // final consecutive charge, resets by age, resets by output, resumes
    // allowed.
    struct Row(&'static str, u64, SocketEnd, BudgetRun);
    let row = |name, life, end, opens, refused, max, last, age, output, resumes| {
        Row(
            name,
            life,
            end,
            BudgetRun {
                opens,
                refused_at_logical_s: refused,
                max_charge: max,
                final_charge: last,
                age_resets: age,
                output_resets: output,
                resumes_allowed: resumes,
            },
        )
    };
    let table = [
        row(
            "real_output_at_threshold_plus_one",
            61,
            RealOutput,
            59,
            None,
            1,
            1,
            0,
            58,
            59,
        ),
        row(
            "idle_at_threshold_plus_one",
            61,
            Idle,
            59,
            None,
            1,
            1,
            58,
            0,
            59,
        ),
        row(
            "briefing_only_at_threshold_plus_one",
            61,
            BriefingOnly,
            59,
            None,
            1,
            1,
            58,
            0,
            59,
        ),
        row(
            "owed_reply_at_threshold_plus_one",
            61,
            OwedReply,
            8,
            Some(549),
            8,
            8,
            0,
            0,
            8,
        ),
        row(
            "idle_at_exact_threshold",
            60,
            Idle,
            59,
            None,
            1,
            1,
            58,
            0,
            59,
        ),
        row(
            "real_output_below_threshold",
            59,
            RealOutput,
            59,
            None,
            1,
            1,
            0,
            58,
            30,
        ),
        row(
            "idle_below_threshold",
            59,
            Idle,
            8,
            Some(531),
            8,
            8,
            0,
            0,
            1,
        ),
        row(
            "briefing_only_below_threshold",
            59,
            BriefingOnly,
            8,
            Some(531),
            8,
            8,
            0,
            0,
            1,
        ),
        row(
            "owed_reply_below_threshold",
            59,
            OwedReply,
            8,
            Some(531),
            8,
            8,
            0,
            0,
            1,
        ),
    ];
    assert_eq!(table.len(), 9);
    for Row(name, life, end, expected) in table {
        assert!(59 * life <= 3600, "{name}");
        assert_eq!(run_restart_budget(59, life, end), expected, "{name}");
    }
}

// Pending tool work across a replacement.
//
// One call, `log_hint` volunteered, is the whole tool effect: executing it once
// moves `volunteered_hints` from 0 to 1, so the counter is the side-effect
// count. The wire counters are the ids of the `toolResponse` frames each socket
// received. A tool continuation is owed, never carried: a response goes to the
// socket that asked for it and to no other, and what survives a replacement is
// the reply the candidate is still owed.

fn tool_call(id: &str) -> crate::gemini::GeminiFunctionCall {
    crate::gemini::GeminiFunctionCall {
        id: id.into(),
        name: crate::runtime::TOOL_LOG_HINT.into(),
        args: serde_json::json!({"requested": false}),
    }
}

fn tool_response_ids(frames: &[serde_json::Value]) -> Vec<String> {
    frames
        .iter()
        .filter_map(|frame| frame.pointer("/toolResponse/functionResponses"))
        .filter_map(serde_json::Value::as_array)
        .flatten()
        .filter_map(|response| response["id"].as_str().map(str::to_owned))
        .collect()
}

fn briefing_turns(frames: &[serde_json::Value]) -> Vec<bool> {
    frames
        .iter()
        .filter_map(|frame| frame.pointer("/clientContent/turnComplete"))
        .filter_map(serde_json::Value::as_bool)
        .collect()
}

impl Rig {
    /// Delivers one tool call from the live socket, as the room loop would.
    async fn receive_tool_call(&mut self, id: &str) {
        let mut context = self.turn.context(
            &mut self.output,
            &mut self.gemini,
            &mut self.board,
            &mut self.media,
        );
        session::handle_gemini_event(
            &self.room,
            &mut context,
            crate::gemini::GeminiEvent::ToolCall(vec![tool_call(id)]),
            Interruptible::Yes,
        )
        .await
        .unwrap();
    }

    /// Joins the old socket's peer and returns every frame it received.
    async fn old_frames(&mut self) -> Vec<serde_json::Value> {
        self.gemini.shutdown().await.unwrap();
        tokio::time::timeout(Duration::from_secs(2), self.old.take().unwrap())
            .await
            .unwrap()
            .unwrap()
    }
}

/// Like `scripted_fresh_gemini`, but a ready socket keeps what it is sent.
async fn recording_fresh_gemini(
    listener: tokio::net::TcpListener,
    script: Vec<FreshStep>,
) -> Vec<(Option<String>, Vec<serde_json::Value>)> {
    let mut seen = Vec::new();
    for step in script {
        let (socket, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
        let setup = socket.next().await.unwrap().unwrap();
        let setup: serde_json::Value = serde_json::from_str(setup.to_text().unwrap()).unwrap();
        let handle = setup["setup"]["sessionResumption"]["handle"]
            .as_str()
            .map(str::to_owned);
        let mut frames = Vec::new();
        match step {
            FreshStep::DropAfterSetup => drop(socket),
            FreshStep::Ready => {
                socket
                    .send(Message::Text(r#"{"setupComplete":{}}"#.into()))
                    .await
                    .unwrap();
                while let Some(Ok(frame)) = socket.next().await {
                    if frame.is_close() {
                        break;
                    }
                    frames.push(serde_json::from_str(frame.to_text().unwrap()).unwrap());
                }
            }
            _ => unreachable!("the tool matrix scripts only ready and dropped sockets"),
        }
        seen.push((handle, frames));
    }
    seen
}

#[derive(Clone, Copy, Debug)]
enum ToolPath {
    /// The result went out on the live socket before the replacement began.
    AcceptedBeforeRotation,
    /// The result write failed, so Gemini was never given it.
    UncertainSend,
    /// As the first, and the resume attempt is refused.
    AcceptedThenRefusedResume,
    /// As the first, with the generation already over, so the continuation
    /// flag is the only debt the replacement can read.
    ContinuationFlagAlone,
}

// Declared before the first run. `effect` is `volunteered_hints`; `old` and
// `fresh` are the toolResponse ids each socket received; `charges` is the
// consecutive-restart count after the replacement.
//
//   path                       effect  old         fresh           charges
//   accepted before rotation   1       [call-1]    [] resumed      1
//   uncertain result send      1       []          [] resumed      1
//   accepted, resume refused   1       [call-1]    [] / [] cold    1
//   continuation flag alone    1       [call-1]    [] resumed      1
//
// Exactly one briefing on the socket that took over asks for the reply, and it
// is the last briefing sent: the accepted result leaves a continuation the old
// socket will never deliver, and the failed write leaves the generation
// unanswered. Neither result is resent, and neither executes twice.
#[tokio::test(flavor = "current_thread")]
async fn pending_tool_work_is_owed_but_never_resent_across_a_replacement() {
    if isolated("pending_tool_work_is_owed_but_never_resent_across_a_replacement") {
        return;
    }
    let paths = [
        ToolPath::AcceptedBeforeRotation,
        ToolPath::UncertainSend,
        ToolPath::AcceptedThenRefusedResume,
        ToolPath::ContinuationFlagAlone,
    ];
    for path in paths {
        eprintln!("tool path: {path:?}");
        let checkpoint = serde_json::json!({"resumable": true, "newHandle": "tool-handle"});
        let (fresh, endpoint) = listener().await;
        let mut rig =
            Rig::with_old_resumption("ws://127.0.0.1:9/".into(), Some(11), Some(checkpoint)).await;
        rig.endpoint = endpoint;
        let old_reader = rig.gemini.inspect_reader_for_test().unwrap();
        wait_for_received_usage(&old_reader).await;
        if matches!(path, ToolPath::UncertainSend) {
            // A closed writer fails the response write deterministically; the
            // call still runs. This is the failed-write case `on_tool_calls`
            // documents, not a peer found dead through the reader.
            rig.gemini.shutdown().await.unwrap();
        }
        rig.receive_tool_call("call-1").await;
        assert_eq!(rig.turn.state.volunteered_hints, 1, "{path:?}");
        let accepted = !matches!(path, ToolPath::UncertainSend);
        assert_eq!(
            rig.turn.activity.socket.tool_response_outstanding, accepted,
            "{path:?}"
        );
        assert!(rig.turn.activity.generating, "{path:?}");
        if matches!(path, ToolPath::ContinuationFlagAlone) {
            rig.turn.activity.generating = false;
            assert!(rig.turn.activity.awaiting_reply_since.is_none());
            assert!(!rig.turn.activity.owes_prompt());
        }
        let script = if matches!(path, ToolPath::AcceptedThenRefusedResume) {
            vec![FreshStep::DropAfterSetup, FreshStep::Ready]
        } else {
            vec![FreshStep::Ready]
        };
        let server = tokio::spawn(recording_fresh_gemini(fresh, script));
        let flow = rig.run().await;
        assert!(flow.is_continue(), "{path:?}");
        assert_eq!(rig.loops.restarts, 1, "{path:?}");
        assert_eq!(rig.turn.state.volunteered_hints, 1, "{path:?}");
        assert!(
            !rig.turn.activity.socket.tool_response_outstanding,
            "{path:?}: the continuation belongs to the retired socket"
        );
        assert_eq!(rig.turn.activity.live_socket, 2, "{path:?}");
        let old = rig.old_frames().await;
        assert_eq!(
            tool_response_ids(&old),
            if accepted {
                vec!["call-1".to_string()]
            } else {
                Vec::new()
            },
            "{path:?}"
        );
        let seen = server.await.unwrap();
        for (_, frames) in &seen {
            assert!(tool_response_ids(frames).is_empty(), "{path:?}: resent");
        }
        let (handle, frames) = seen.last().unwrap();
        match path {
            ToolPath::AcceptedThenRefusedResume => {
                assert_eq!(seen.len(), 2);
                assert_eq!(seen[0].0.as_deref(), Some("tool-handle"));
                assert_eq!(*handle, None, "{path:?}: the fallback is cold");
            }
            _ => {
                assert_eq!(seen.len(), 1);
                assert_eq!(handle.as_deref(), Some("tool-handle"), "{path:?}");
            }
        }
        let turns = briefing_turns(frames);
        assert_eq!(turns.last(), Some(&true), "{path:?}");
        assert_eq!(turns.iter().filter(|asked| **asked).count(), 1, "{path:?}");
    }
}

#[derive(Clone, Copy, Debug)]
enum ToolStage {
    /// The resume open is in flight when the terminal arrives.
    ResumePending,
    /// The cold open was refused and its retry backoff is running.
    ColdBackoff,
}

// Declared before the first run, for every (stage, stop, result) row:
//
//   stage          charges  tool effect  old toolResponse ids      fresh frames
//   resume pending 1        1            [call-1] / [] uncertain   none
//   cold backoff   2        1            [call-1] / [] uncertain   none
//
// The terminal is End or the original deadline. Both break the room loop with
// the terminal intact, so the report path is the one the stop already chose.
// The reply the candidate was owed is set down as a required prompt before the
// first cancellable wait and the stop must not undo it; the continuation flag
// is cleared there too, because the socket it belonged to is gone and no live
// socket will deliver it. These two checks therefore guard against the stop
// undoing them, not against the stop doing the work. Nothing is written after
// the stop, no further open is attempted, and the deadline is not extended.
async fn tool_cancel_matrix(stage: ToolStage) {
    let mut rows = 0;
    for stop in [OpenStop::End, OpenStop::Deadline] {
        for accepted in [true, false] {
            let label = format!("{stage:?} {stop:?} accepted={accepted}");
            let checkpoint = matches!(stage, ToolStage::ResumePending)
                .then(|| serde_json::json!({"resumable": true, "newHandle": "tool-handle"}));
            let (listener, endpoint) = listener().await;
            let mut rig =
                Rig::with_old_resumption("ws://127.0.0.1:9/".into(), Some(11), checkpoint).await;
            rig.endpoint = endpoint;
            let old_reader = rig.gemini.inspect_reader_for_test().unwrap();
            wait_for_received_usage(&old_reader).await;
            if !accepted {
                rig.gemini.shutdown().await.unwrap();
            }
            rig.receive_tool_call("call-1").await;
            assert_eq!(rig.turn.state.volunteered_hints, 1, "{label}");
            rig.loops.deadline_at = tokio::time::Instant::now()
                + if matches!(stage, ToolStage::ColdBackoff) {
                    Duration::from_secs(1)
                } else {
                    Duration::from_secs(5)
                };
            let deadline = rig.loops.deadline_at;
            let (probe, mut observed) = mpsc::unbounded_channel();
            rig.loops.recovery_terminal = Some(probe);
            let (unused, _) = mpsc::unbounded_channel();
            let mut room = Some(std::mem::replace(&mut rig._room_sender, unused));
            let presence_now = std::sync::Arc::new(std::sync::Mutex::new(Instant::now()));
            let (entered, pending) = oneshot::channel();
            let server = tokio::spawn(async move {
                use tokio::io::{AsyncReadExt, AsyncWriteExt};
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                while !request.ends_with(b"\r\n\r\n") {
                    assert!(request.len() < 4096);
                    request.push(socket.read_u8().await.unwrap());
                }
                match stage {
                    ToolStage::ResumePending => {
                        entered.send(()).unwrap();
                        closed_tcp(socket).await;
                    }
                    ToolStage::ColdBackoff => {
                        socket
                            .write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                            .await
                            .unwrap();
                        drop(socket);
                        entered.send(()).unwrap();
                    }
                }
                listener
            });
            let tx = rig.tx.clone();
            let recorder = rig.recorder.clone();
            let stimulus = async {
                pending.await.unwrap();
                if matches!(stage, ToolStage::ColdBackoff) {
                    let until = tokio::time::timeout(Duration::from_secs(1), async {
                        loop {
                            let mut deadlines = recorder.take_retry_backoffs();
                            if let Some(until) = deadlines.pop() {
                                break until;
                            }
                            tokio::task::yield_now().await;
                        }
                    })
                    .await
                    .unwrap();
                    assert!(deadline < until, "{label}: the stop must beat the backoff");
                }
                stop_pending_open(stop, deadline, &tx, &mut room, &mut observed, &presence_now)
                    .await;
            };
            let (flow, ()) = tokio::join!(rig.run(), stimulus);
            assert!(flow.is_break(), "{label}");
            assert_eq!(rig.loops.deadline_at, deadline, "{label}");
            assert_eq!(
                rig.loops.restarts,
                if matches!(stage, ToolStage::ColdBackoff) {
                    2
                } else {
                    1
                },
                "{label}: the charge survives the stop"
            );
            assert_eq!(rig.turn.state.volunteered_hints, 1, "{label}");
            assert!(rig.turn.activity.owes_prompt(), "{label}: obligation kept");
            assert!(
                !rig.turn.activity.socket.tool_response_outstanding,
                "{label}"
            );
            assert_eq!(rig.turn.activity.live_socket, 1, "{label}");
            assert!(
                rig.gemini
                    .send_text("no socket may take this")
                    .await
                    .is_err(),
                "{label}: nothing is installed after the stop"
            );
            assert!(observed.try_recv().is_err(), "{label}: one terminal");
            assert!(
                !rig.recorder.take().iter().any(|record| matches!(record,
                    room_link::Sent::Data { topic, .. } if topic.as_deref() == Some(crate::runtime::TOPIC_REPORT))),
                "{label}: the stop sends no report itself"
            );
            let old = rig.old_frames().await;
            assert_eq!(
                tool_response_ids(&old),
                if accepted {
                    vec!["call-1".to_string()]
                } else {
                    Vec::new()
                },
                "{label}"
            );
            let listener = server.await.unwrap();
            assert_eq!(
                listener.into_std().unwrap().accept().unwrap_err().kind(),
                std::io::ErrorKind::WouldBlock,
                "{label}: no further open"
            );
            rows += 1;
        }
    }
    assert_eq!(rows, 4);
}

#[tokio::test(flavor = "current_thread")]
async fn pending_tool_work_survives_end_and_deadline_during_a_resume_open() {
    if isolated("pending_tool_work_survives_end_and_deadline_during_a_resume_open") {
        return;
    }
    tool_cancel_matrix(ToolStage::ResumePending).await;
}

#[tokio::test(flavor = "current_thread")]
async fn pending_tool_work_survives_end_and_deadline_during_cold_backoff() {
    if isolated("pending_tool_work_survives_end_and_deadline_during_cold_backoff") {
        return;
    }
    tool_cancel_matrix(ToolStage::ColdBackoff).await;
}

// Declared before the first run. A graceful restart is held while a tool
// continuation is outstanding, even when the candidate has started speaking:
// nothing is retired, nothing is charged, the held restart stays armed, and the
// continuation stays on the live socket that owes it. Expected: the loop
// continues, 0 charges, 1 socket, the restart still armed, the flag still up,
// no frame beyond setup on the live socket, and no tool effect.
#[tokio::test(flavor = "current_thread")]
async fn a_deferred_restart_holds_while_a_tool_continuation_is_outstanding() {
    if isolated("a_deferred_restart_holds_while_a_tool_continuation_is_outstanding") {
        return;
    }
    let mut rig = Rig::new("ws://127.0.0.1:9/".into()).await;
    rig.prefix = true;
    rig.media.audio_bytes = vec![1, 2];
    rig.media.identity = Some("candidate".into());
    rig.loops.deferred_restart = DeferredRestart(true);
    rig.turn
        .activity
        .note_candidate_voice_activity(true, Instant::now());
    rig.turn.activity.note_tool_response(Instant::now());
    let flow = rig.run().await;
    assert!(flow.is_continue());
    assert_eq!(rig.loops.restarts, 0);
    assert_eq!(rig.turn.activity.live_socket, 1);
    assert!(rig.loops.deferred_restart.is_armed());
    assert!(rig.turn.activity.socket.tool_response_outstanding);
    assert_eq!(rig.turn.state.volunteered_hints, 0);
    assert_eq!(rig.media.audio_bytes, vec![1, 2], "no audio was consumed");
    rig.finish().await;
}

// Declared before the first run. With no continuation outstanding the speech
// prefix runs, and during it the old socket issues `call-2`. Expected: it is
// discarded and counted (discarded_events=1), never executed (effect 0) and
// never answered (no toolResponse on either socket); the transcript fragment is
// kept; the replacement is a single charge; no continuation flag is raised on
// the new socket.
#[tokio::test(flavor = "current_thread")]
async fn a_tool_call_arriving_during_the_speech_prefix_is_discarded_not_executed() {
    if isolated_checked(
        "a_tool_call_arriving_during_the_speech_prefix_is_discarded_not_executed",
        |output| {
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(
                stderr.lines().any(|line| line
                    .starts_with("speech handoff: prefix_fragments=1 discarded_events=1 ")),
                "the call must be counted as discarded: {stderr}"
            );
        },
    ) {
        return;
    }
    let mut rig = Rig::new("ws://127.0.0.1:9/".into()).await;
    rig.gemini.shutdown().await.unwrap();
    assert!(rig.old.take().unwrap().await.unwrap().is_empty());
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
        let mut frames = Vec::new();
        let end = socket.next().await.unwrap().unwrap();
        frames.push(serde_json::from_str::<serde_json::Value>(end.to_text().unwrap()).unwrap());
        assert_eq!(frames[0]["realtimeInput"]["audioStreamEnd"], true);
        let call = serde_json::json!({"toolCall": {"functionCalls": [
            {"id": "call-2", "name": "log_hint", "args": {"requested": false}}
        ]}});
        socket
            .send(Message::Text(call.to_string().into()))
            .await
            .unwrap();
        let speech = serde_json::json!({
            "serverContent": {"inputTranscription": {"text": PREFIX_TEXT}},
            "usageMetadata": {"promptTokenCount": 42},
        });
        socket
            .send(Message::Text(speech.to_string().into()))
            .await
            .unwrap();
        while let Some(frame) = socket.next().await {
            match frame {
                Ok(frame) if frame.is_close() => break,
                Ok(frame) => frames.push(serde_json::from_str(frame.to_text().unwrap()).unwrap()),
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
        frames
    });
    let boot = crate::runtime::bootstrap(&rig.config, "recovery-test", Some("two-sum"), 45);
    rig.gemini = crate::gemini::live_session_with_keys_at(&old_endpoint, &rig.keys, &boot, None)
        .await
        .unwrap();
    rig.old = Some(old);
    rig.expected_old_messages = 2;
    let (fresh, endpoint) = listener().await;
    rig.endpoint = endpoint;
    let server = tokio::spawn(recording_fresh_gemini(fresh, vec![FreshStep::Ready]));
    rig.prefix = true;
    rig.media.audio_bytes = vec![1, 2];
    rig.media.audio_unconfirmed = true;
    rig.media.identity = Some("candidate".into());
    rig.loops.deferred_restart = DeferredRestart(true);
    rig.turn
        .activity
        .note_candidate_voice_activity(true, Instant::now());
    let flow = rig.run().await;
    assert!(flow.is_continue());
    assert_eq!(rig.loops.restarts, 1);
    assert_eq!(rig.turn.state.volunteered_hints, 0, "call-2 never ran");
    assert!(!rig.turn.activity.socket.tool_response_outstanding);
    assert_eq!(rig.turn.activity.live_socket, 2);
    let old = rig.old_frames().await;
    assert_eq!(old.len(), 2, "end of audio and the listening note only");
    assert!(tool_response_ids(&old).is_empty());
    let seen = server.await.unwrap();
    assert_eq!(seen.len(), 1);
    assert!(tool_response_ids(&seen[0].1).is_empty());
    assert!(
        rig.turn
            .state
            .transcript
            .iter()
            .any(|line| line.contains(PREFIX_TEXT)),
        "the prefix transcript is kept"
    );
}

// Non-terminal races on a replacement's final sends, and what replace itself
// owes the room when the replacement fails.

fn test_result_packet(passed: u32, code: &str) -> Packet {
    Packet {
        topic: Some(crate::runtime::TOPIC_TEST_RESULTS.into()),
        sender: Some("candidate".into()),
        payload: serde_json::to_vec(&serde_json::json!({
            "passed": passed, "total": 40, "code": code, "language": "python"
        }))
        .unwrap(),
    }
}

fn code_packet(code: &str) -> Packet {
    Packet {
        sender: Some("candidate".into()),
        topic: Some(crate::runtime::TOPIC_CODE_UPDATE.into()),
        payload: serde_json::to_vec(&serde_json::json!({"code": code, "language": "python"}))
            .unwrap(),
    }
}

fn usage_lines(output: &std::process::Output) -> Vec<String> {
    String::from_utf8_lossy(&output.stderr)
        .lines()
        .filter(|line| line.starts_with("codetrial live_turn_usage "))
        .map(str::to_owned)
        .collect()
}

fn has_field(line: &str, field: &str) -> bool {
    line.split_whitespace().any(|value| value == field)
}

// Declared before running: a Billing refusal of the only key fails the
// replacement after the room's queued work was reduced into the driver but
// before any notice or cleanup could run. replace must hand the round back to
// the loop and count the evictions it never performed.
#[tokio::test(flavor = "current_thread")]
async fn a_failed_replacement_hands_room_work_back_through_replace() {
    if isolated_checked(
        "a_failed_replacement_hands_room_work_back_through_replace",
        |output| {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let ingress = stderr
                .lines()
                .filter(|line| line.starts_with("recovery ingress: "))
                .collect::<Vec<_>>();
            assert_eq!(ingress.len(), 1, "{stderr}");
            for field in [
                "omitted_evictions=2",
                "omitted_contexts=0",
                "dropped_bytes=0",
            ] {
                assert!(has_field(ingress[0], field), "{field}: {}", ingress[0]);
            }
            let usage = usage_lines(output);
            assert_eq!(usage.len(), 1, "old usage drains once: {usage:?}");
            assert!(has_field(&usage[0], "socket=1") && has_field(&usage[0], "cause=tool"));
        },
    ) {
        return;
    }
    let (fresh, endpoint) = listener().await;
    let mut rig = Rig::with_old_usage(endpoint, Some(11)).await;
    rig.gemini.input_cause = Some("tool");
    let old = rig.gemini.inspect_reader_for_test().unwrap();
    wait_for_received_usage(&old).await;
    rig.turn.state.coding_minutes = 0;
    rig.turn.state.judged_code = rig.turn.state.code.clone();
    rig.turn.state.judged_language = rig.turn.state.language.clone();
    assert!(!crate::agent::phase_judge_due(&rig.turn.state));
    rig.recorder.seed_evictions(&["duplicate-a", "duplicate-b"]);
    control(
        &rig.tx,
        serde_json::json!({"type":"round_transition", "round":"behavioral"}),
    );
    let server = tokio::spawn(scripted_fresh_gemini(
        fresh,
        vec![FreshStep::Refuse(axum::http::StatusCode::PAYMENT_REQUIRED)],
    ));
    let flow = rig.run().await;
    assert!(flow.is_break());
    assert_eq!(rig.turn.activity.live_exit, LiveOutcome::Billing);
    assert!(old.is_finished());
    assert_eq!(old.pending_usage_count(), 0);
    assert_eq!(
        rig.loops.pending_round_notice,
        Some("skipped"),
        "the round the candidate caused survives the failed replacement"
    );
    let sent = rig.recorder.take();
    assert!(
        !sent.iter().any(|sent| matches!(
            sent,
            room_link::Sent::Data { message, .. } if message["type"] == "round_state"
        )),
        "no notice was published by the failed replacement"
    );
    rig.finish().await;
    let (seen, _) = server.await.unwrap();
    assert_eq!(seen.len(), 1);
}

// The fresh peer for a gated final send. It records what reaches the wire and,
// when told, drops the connection without a closing handshake so the real
// reader finishes and the next write takes the Err branch.
async fn closable_fresh_peer(
    listener: tokio::net::TcpListener,
    usage: u64,
    close: oneshot::Receiver<usize>,
) -> Vec<serde_json::Value> {
    let (socket, _) = listener.accept().await.unwrap();
    let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
    socket.next().await.unwrap().unwrap();
    socket
        .send(Message::Text(r#"{"setupComplete":{}}"#.into()))
        .await
        .unwrap();
    socket
        .send(Message::Text(
            serde_json::json!({"usageMetadata": {"promptTokenCount": usage}})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
    let mut messages = Vec::new();
    let mut close = Some(close);
    loop {
        tokio::select! {
            frame = socket.next() => match frame {
                Some(Ok(frame)) if frame.is_close() => break,
                Some(Ok(frame)) => {
                    messages.push(serde_json::from_str(frame.to_text().unwrap()).unwrap())
                }
                _ => break,
            },
            dropped = async { close.as_mut().unwrap().await }, if close.is_some() => {
                close = None;
                if let Ok(prior) = dropped {
                    // Drop only after the frames written before the failure
                    // were read, so a reset cannot discard them unseen.
                    while messages.len() < prior {
                        let frame = tokio::time::timeout(Duration::from_secs(2), socket.next())
                            .await
                            .expect("the prior writes reach the peer")
                            .unwrap()
                            .unwrap();
                        messages.push(serde_json::from_str(frame.to_text().unwrap()).unwrap());
                    }
                    drop(socket);
                    break;
                }
            }
        }
    }
    messages
}

struct GatedRun {
    flow: ControlFlow<()>,
    fresh: crate::gemini::LiveReaderInspection,
    server: tokio::task::JoinHandle<Vec<serde_json::Value>>,
    failed: bool,
}

impl GatedRun {
    // Releases the committed or dead fresh socket and the retired one, and
    // returns what the fresh peer received.
    async fn finish(self, mut rig: Rig) -> Vec<serde_json::Value> {
        if self.failed {
            drop(rig.gemini.retire());
        } else {
            rig.gemini.shutdown().await.unwrap();
        }
        let wire = tokio::time::timeout(Duration::from_secs(2), self.server)
            .await
            .unwrap()
            .unwrap();
        drop(rig.gemini.retire());
        rig.finish().await;
        wire
    }
}

// Holds the labelled write of the real replacement, runs `during` while it is
// held, then either releases it or first drops the peer and waits for the real
// reader to finish. The fresh socket's received usage is observed before the
// release and is never drained here.
async fn gated_final_send(
    rig: &mut Rig,
    listener: tokio::net::TcpListener,
    phase: &'static str,

    // None releases the write; Some(n) drops the peer after it read the n
    // frames written before the gate, so the held write fails.
    fail_after: Option<usize>,
    during: impl FnOnce(&mpsc::UnboundedSender<Packet>),
) -> GatedRun {
    let fail = fail_after.is_some();
    let entered = std::sync::Arc::new(tokio::sync::Notify::new());
    let release = std::sync::Arc::new(tokio::sync::Notify::new());
    rig.loops.recovery_writes.push_back(WriteBarrier {
        phase,
        entered: entered.clone(),
        release: release.clone(),
    });
    let (reader, mut readers) = mpsc::unbounded_channel();
    rig.loops.recovery_reader = Some(reader);
    let (close, closed) = oneshot::channel();
    let server = tokio::spawn(closable_fresh_peer(listener, 29, closed));
    let tx = rig.tx.clone();
    let mut close = Some(close);
    let stimulus = async {
        entered.notified().await;
        let fresh = readers.recv().await.unwrap();
        wait_for_received_usage(&fresh).await;
        assert!(!fresh.is_finished());
        during(&tx);
        if fail {
            close.take().unwrap().send(fail_after.unwrap()).unwrap();
            tokio::time::timeout(Duration::from_secs(2), async {
                while !fresh.is_finished() {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .expect("the dropped peer ends the real reader");
            assert_eq!(fresh.pending_usage_count(), 1);
        }
        release.notify_one();
        fresh
    };
    let (flow, fresh) = tokio::join!(rig.run(), stimulus);
    GatedRun {
        flow,
        fresh,
        server,
        failed: fail,
    }
}

fn session_usage(rig: &mut Rig) {
    let mut context = rig.turn.context(
        &mut rig.output,
        &mut rig.gemini,
        &mut rig.board,
        &mut rig.media,
    );
    session::drain_live_usage(&rig.room, &mut context);
}

// Declared before running. The peer drops the fresh socket while the briefing
// is held, after the candidate's input reached the driver. The briefing then
// fails on its own Err branch, and so must the held-context or action write
// that drain attempts next. Nothing may be settled: no reference named, the
// Thinking debt and the owed prompt remain, the cold brief is demanded again,
// the edit is not recorded as shown, and the test fact stays recorded. The peer
// sees no frame. Old usage drains once at replacement (socket 1, tool); the
// dead fresh socket's usage, received before the failure, is still queued
// afterwards, is drained exactly once by the loop's own drain (socket 2), and
// carries the cause of the last send that was attempted on it: a silent context
// write leaves the candidate cause, a failed reply request leaves recovery.
//   row 0: the hold stays and a test result is held -> flush_contexts Err.
//   row 1: the hold is released and a test result arrives -> flush_actions Err.
#[tokio::test(flavor = "current_thread")]
async fn a_failed_final_send_settles_nothing_it_did_not_deliver() {
    if isolated_checked(
        "a_failed_final_send_settles_nothing_it_did_not_deliver",
        |output| {
            let usage = usage_lines(output);
            assert_eq!(usage.len(), 4, "{usage:?}");
            for (row, cause) in [(0, "candidate"), (1, "recovery")] {
                let (old, fresh) = (&usage[row * 2], &usage[row * 2 + 1]);
                for field in ["socket=1", "cause=tool", "prompt_tokens=11"] {
                    assert!(has_field(old, field), "{old}");
                }
                for field in [
                    "socket=2".to_owned(),
                    format!("cause={cause}"),
                    "prompt_tokens=29".to_owned(),
                ] {
                    assert!(has_field(fresh, &field), "{field}: {fresh}");
                }
            }
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert_eq!(
                stderr
                    .lines()
                    .filter(|line| line.starts_with("recovery ingress: "))
                    .count(),
                2
            );
        },
    ) {
        return;
    }
    for release in [false, true] {
        let (listener, endpoint) = listener().await;
        let mut rig = Rig::with_old_usage(endpoint, Some(11)).await;
        rig.gemini.input_cause = Some("tool");
        let old = rig.gemini.inspect_reader_for_test().unwrap();
        wait_for_received_usage(&old).await;
        rig.turn.state.thinking_hold = crate::agent::ThinkingHold::Held {
            since: Instant::now(),
        };
        rig.turn.state.owed_reply_on_resume =
            Some(Some("[SYSTEM EVENT] Answer the original question.".into()));
        rig.turn
            .activity
            .owe_prompt(Instant::now(), Some("Answer the original question.".into()));
        rig.turn.state.earlier_steps_pending.push("design");
        rig.turn.state.code = "def solve(): return 7".into();
        rig.turn.state.language = "python".into();
        rig.turn.activity.last_test_reaction = Instant::now() - Duration::from_secs(100);

        // The second write's own gate proves the Err branch under test ran: it
        // is consumed only if drain really attempted that write.
        let next = if release {
            "held_action"
        } else {
            "held_context"
        };
        let (entered, open) = (
            std::sync::Arc::new(tokio::sync::Notify::new()),
            std::sync::Arc::new(tokio::sync::Notify::new()),
        );
        rig.loops.recovery_writes.push_back(WriteBarrier {
            phase: next,
            entered: entered.clone(),
            release: open.clone(),
        });
        tokio::spawn(async move {
            entered.notified().await;
            open.notify_one();
        });
        let run = gated_final_send(&mut rig, listener, "recovery_brief", Some(0), |tx| {
            if release {
                control(tx, serde_json::json!({"type":"thinking", "thinking":false}));
            }
            tx.send(test_result_packet(3, "def solve(): return 7"))
                .unwrap();
            tx.send(code_packet("def late_editor_marker(): pass"))
                .unwrap();
        })
        .await;
        assert!(run.flow.is_continue(), "release={release}");
        assert!(old.is_finished());
        assert_eq!(old.pending_usage_count(), 0);
        let state = &rig.turn.state;
        assert!(state.needs_cold_brief, "release={release}");
        assert_eq!(state.test_runs, 1);
        assert!(state.code.contains("late_editor_marker"));
        assert!(
            !state.code_shown.contains("late_editor_marker"),
            "an edit the model never received is not shown"
        );
        assert_eq!(state.earlier_steps_pending, ["design"]);
        assert!(state.earlier_steps_named.is_empty());
        assert!(
            state.owed_reply_on_resume.is_some(),
            "a failed send cannot settle the debt: release={release}"
        );
        assert_eq!(state.interviewer_silenced(), !release);
        assert!(rig.turn.activity.owes_prompt());
        assert!(rig.loops.recovery_writes.is_empty());
        assert_eq!(run.fresh.pending_usage_count(), 1);
        session_usage(&mut rig);
        session_usage(&mut rig);
        assert_eq!(run.fresh.pending_usage_count(), 0);
        let thinking = rig
            .recorder
            .take()
            .into_iter()
            .filter_map(|sent| match sent {
                room_link::Sent::Data { message, .. } if message["type"] == "thinking_state" => {
                    Some(message["thinking"].as_bool().unwrap())
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            thinking.last().copied(),
            // An unchanged hold announces nothing; a release is published even
            // though the write after it failed.
            release.then_some(false),
            "the latest Thinking notice survives the failed write: {thinking:?}"
        );
        assert!(run.finish(rig).await.is_empty());
    }
}

struct FinalSendRow {
    name: &'static str,
    phase: &'static str,

    // Thinking is held with a raw debt and a test summary already queued, so
    // the briefing is silent and drain has a held context to send.
    held: bool,

    // Release Thinking while the held context is on the wire, so the action is
    // the next write.
    release_in_context: bool,
    during: fn(&mpsc::UnboundedSender<Packet>),
}

const LATE_EDIT: &str = "def late_editor_marker(): pass";

// Frames that ask the model to answer: a completed turn, or realtime text that
// a busy socket answers after its current reply.
fn complete_frames(wire: &[serde_json::Value]) -> usize {
    wire.iter()
        .filter(|frame| {
            frame["clientContent"]["turnComplete"] == true
                || frame["realtimeInput"]["text"].is_string()
        })
        .count()
}

fn wire_has(wire: &[serde_json::Value], text: &str) -> bool {
    wire.iter().any(|frame| frame.to_string().contains(text))
}

// Declared before running, per row (wire counts are client frames after the
// fresh setup):
//   brief_edit    one reply brief. An edit alone is not an obligation, so the
//                 briefing is not resent and nothing is discarded; the edit is
//                 not marked shown and stays owed to the next reply.
//   brief_test    two reply briefs, the second carrying the test summary and
//                 consuming the action it made; no third send; discard armed.
//   brief_hold    a reply brief, then a silent one under the new hold: three
//                 frames, one reply, discard armed, hold and its notice last.
//   context_edit  a silent brief and the held context; the edit is news the
//                 hold keeps back, so it is neither sent nor marked shown.
//   action_edit   a silent brief, the held context and the released action
//                 (one reply); the action was prepared before the edit, so the
//                 edit is not marked shown, the debt it carried is settled and
//                 no output is discarded.
//   action_test   as action_edit, but a second summary inside the reaction
//                 cooldown is a recorded run and not a second action: one
//                 reply, no output discarded.
//   brief_round   a round request during the briefing is an obligation: a
//                 second briefing follows, the first reply is discarded, and
//                 the round notice is published once.
//   action_round  a round request during the action makes a second action:
//                 two replies, the debt settled by the last, no discard.
#[tokio::test(flavor = "current_thread")]
async fn input_during_a_final_send_settles_only_what_that_send_carried() {
    if isolated_checked(
        "input_during_a_final_send_settles_only_what_that_send_carried",
        |output| {
            let usage = usage_lines(output);
            assert_eq!(usage.len(), 16, "{usage:?}");
            for pair in usage.as_chunks::<2>().0 {
                assert!(has_field(&pair[0], "socket=1") && has_field(&pair[0], "cause=tool"));
                assert!(has_field(&pair[1], "socket=2") && has_field(&pair[1], "prompt_tokens=29"));
            }
        },
    ) {
        return;
    }
    let rows = [
        FinalSendRow {
            name: "brief_edit",
            phase: "recovery_brief",
            held: false,
            release_in_context: false,
            during: |tx| tx.send(code_packet(LATE_EDIT)).unwrap(),
        },
        FinalSendRow {
            name: "brief_test",
            phase: "recovery_brief",
            held: false,
            release_in_context: false,
            during: |tx| {
                tx.send(test_result_packet(3, "def solve(): return 7"))
                    .unwrap()
            },
        },
        FinalSendRow {
            name: "brief_hold",
            phase: "recovery_brief",
            held: false,
            release_in_context: false,
            during: |tx| control(tx, serde_json::json!({"type":"thinking", "thinking":true})),
        },
        FinalSendRow {
            name: "context_edit",
            phase: "held_context",
            held: true,
            release_in_context: false,
            during: |tx| tx.send(code_packet(LATE_EDIT)).unwrap(),
        },
        FinalSendRow {
            name: "action_edit",
            phase: "held_action",
            held: true,
            release_in_context: true,
            during: |tx| tx.send(code_packet(LATE_EDIT)).unwrap(),
        },
        FinalSendRow {
            name: "brief_round",
            phase: "recovery_brief",
            held: false,
            release_in_context: false,
            during: |tx| {
                control(
                    tx,
                    serde_json::json!({"type":"round_transition", "round":"behavioral"}),
                )
            },
        },
        FinalSendRow {
            name: "action_round",
            phase: "held_action",
            held: true,
            release_in_context: true,
            during: |tx| {
                control(
                    tx,
                    serde_json::json!({"type":"round_transition", "round":"behavioral"}),
                )
            },
        },
        FinalSendRow {
            name: "action_test",
            phase: "held_action",
            held: true,
            release_in_context: true,
            during: |tx| {
                tx.send(test_result_packet(5, "def solve(): return 7"))
                    .unwrap()
            },
        },
    ];
    for row in rows {
        eprintln!("final send row: {}", row.name);
        let (listener, endpoint) = listener().await;
        let mut rig = Rig::with_old_usage(endpoint, Some(11)).await;
        rig.gemini.input_cause = Some("tool");
        let old = rig.gemini.inspect_reader_for_test().unwrap();
        wait_for_received_usage(&old).await;
        rig.turn.state.code = "def solve(): return 7".into();
        rig.turn.state.language = "python".into();
        rig.turn.activity.last_test_reaction = Instant::now() - Duration::from_secs(100);
        rig.turn.state.coding_minutes = 0;
        rig.turn.state.judged_code = rig.turn.state.code.clone();
        rig.turn.state.judged_language = rig.turn.state.language.clone();
        rig.turn.state.owed_reply_on_resume = Some(Some("[SYSTEM EVENT] Answer.".into()));
        rig.turn
            .activity
            .owe_prompt(Instant::now(), Some("Answer.".into()));
        if row.held {
            rig.turn.state.thinking_hold = crate::agent::ThinkingHold::Held {
                since: Instant::now(),
            };
            rig.tx
                .send(test_result_packet(0, "def solve(): return 7"))
                .unwrap();
        }
        if row.release_in_context {
            let (entered, open) = (
                std::sync::Arc::new(tokio::sync::Notify::new()),
                std::sync::Arc::new(tokio::sync::Notify::new()),
            );
            rig.loops.recovery_writes.push_back(WriteBarrier {
                phase: "held_context",
                entered: entered.clone(),
                release: open.clone(),
            });
            let tx = rig.tx.clone();
            tokio::spawn(async move {
                entered.notified().await;
                control(
                    &tx,
                    serde_json::json!({"type":"thinking", "thinking":false}),
                );
                open.notify_one();
            });
        }
        let run = gated_final_send(&mut rig, listener, row.phase, None, row.during).await;
        assert!(run.flow.is_continue(), "{}", row.name);
        assert!(rig.loops.recovery_writes.is_empty(), "{}", row.name);
        assert!(old.is_finished() && old.pending_usage_count() == 0);
        assert_eq!(run.fresh.pending_usage_count(), 1, "{}", row.name);
        session_usage(&mut rig);
        session_usage(&mut rig);
        assert_eq!(run.fresh.pending_usage_count(), 0);
        let discarding = rig.turn.activity.socket.discarding_output;
        let shown = rig.turn.state.code_shown.contains("late_editor_marker");
        let debt = rig.turn.state.owed_reply_on_resume.is_some();
        let silenced = rig.turn.state.interviewer_silenced();
        let runs = rig.turn.state.test_runs;
        let has_code = rig.turn.state.code.contains("late_editor_marker");
        let round_pending = rig.loops.pending_round_notice;
        let rounds = rig
            .recorder
            .take()
            .into_iter()
            .filter_map(|sent| match sent {
                room_link::Sent::Data { message, .. } if message["type"] == "round_state" => {
                    Some(message["status"].as_str().unwrap().to_owned())
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        // What the next reply would still tell the model about the editor.
        let owed_edit = crate::agent::editor_update_body(&rig.turn.state)
            .is_some_and(|body| body.contains("late_editor_marker"));
        assert_eq!(owed_edit, has_code && !shown, "{}", row.name);
        let wire = run.finish(rig).await;
        let on_wire = wire_has(&wire, "late_editor_marker");
        assert_eq!(shown, on_wire, "{}: shown iff sent: {wire:?}", row.name);
        match row.name {
            "brief_edit" => {
                // An edit alone does not resend the briefing; its delivery
                // stays owed through `code_shown`.
                assert!(has_code && !shown && owed_edit);
                assert_eq!((wire.len(), complete_frames(&wire)), (2, 1));
                assert!(!discarding);
            }
            "brief_test" => {
                assert_eq!(runs, 1);
                assert_eq!((wire.len(), complete_frames(&wire)), (4, 2));
                assert!(wire_has(&wire[3..], "3/40 cases passed"));
                assert!(!wire_has(&wire[..2], "3/40 cases passed"));
                assert!(discarding);
            }
            "brief_hold" => {
                assert_eq!((wire.len(), complete_frames(&wire)), (3, 1));
                assert!(discarding && silenced);
            }
            "context_edit" => {
                assert!(has_code && !shown);
                assert_eq!((wire.len(), complete_frames(&wire)), (2, 0));
                assert!(silenced && !discarding);
            }
            "action_edit" => {
                assert!(has_code && !shown);
                assert_eq!((wire.len(), complete_frames(&wire)), (4, 1));
                assert!(!debt && !silenced && !discarding);
            }
            "action_test" => {
                // The second summary is inside the reaction cooldown, so it is
                // a recorded run and not a second action.
                assert_eq!(runs, 2);
                assert_eq!((wire.len(), complete_frames(&wire)), (4, 1));
                assert!(!debt && !silenced && !discarding);
            }
            "brief_round" => {
                // A round request is an obligation: the first reply is
                // discarded because the corrected briefing follows it.
                assert_eq!((wire.len(), complete_frames(&wire)), (4, 2));
                assert!(discarding && !debt);
                assert_eq!(rounds, ["skipped"]);
            }
            "action_round" => {
                // The first action is not allowed to settle the debt the second
                // one still carries; no output is discarded.
                assert_eq!((wire.len(), complete_frames(&wire)), (6, 2));
                assert!(!debt && !silenced && !discarding);
                assert_eq!(rounds, ["skipped"]);
                assert!(round_pending.is_none());
            }
            other => unreachable!("{other}"),
        }
    }
}

// Declared before running. The fresh peer drops while the targeted held write
// is gated, after newer input reached the driver; the write then takes its Err
// branch.
//   held_context: a release arrives. flush_contexts fails (cold brief owed
//     again, the reference restored, only the briefing on the wire); drain
//     still publishes the latest Thinking notice (released) and does not
//     attempt the action that the release made, so the debt stays unsettled.
//   held_action: a newer hold arrives while the older action is on the wire.
//     The action fails (briefing and held context on the wire, no action),
//     its reference is restored, the debt is not settled, and the newer hold's
//     notice is still published after the failure.
#[tokio::test(flavor = "current_thread")]
async fn a_failed_held_write_keeps_newer_input_and_unsettled_debt() {
    if isolated_checked(
        "a_failed_held_write_keeps_newer_input_and_unsettled_debt",
        |output| {
            let usage = usage_lines(output);
            assert_eq!(usage.len(), 4, "{usage:?}");
            for (row, cause) in [(0, "candidate"), (1, "recovery")] {
                let (old, fresh) = (&usage[row * 2], &usage[row * 2 + 1]);
                assert!(has_field(old, "socket=1") && has_field(old, "cause=tool"));
                assert!(has_field(fresh, "socket=2") && has_field(fresh, "prompt_tokens=29"));
                assert!(has_field(fresh, &format!("cause={cause}")), "{fresh}");
            }
        },
    ) {
        return;
    }
    for phase in ["held_context", "held_action"] {
        let action = phase == "held_action";
        let (listener, endpoint) = listener().await;
        let mut rig = Rig::with_old_usage(endpoint, Some(11)).await;
        rig.gemini.input_cause = Some("tool");
        let old = rig.gemini.inspect_reader_for_test().unwrap();
        wait_for_received_usage(&old).await;
        rig.turn.state.thinking_hold = crate::agent::ThinkingHold::Held {
            since: Instant::now(),
        };
        rig.turn.state.owed_reply_on_resume = Some(Some("[SYSTEM EVENT] Answer.".into()));
        rig.turn
            .activity
            .owe_prompt(Instant::now(), Some("Answer.".into()));
        rig.turn.state.earlier_steps_pending.push("design");
        rig.turn.state.code = "def solve(): return 7".into();
        rig.turn.state.language = "python".into();
        rig.turn.activity.last_test_reaction = Instant::now() - Duration::from_secs(100);
        rig.tx
            .send(test_result_packet(0, "def solve(): return 7"))
            .unwrap();
        if action {
            let (entered, open) = (
                std::sync::Arc::new(tokio::sync::Notify::new()),
                std::sync::Arc::new(tokio::sync::Notify::new()),
            );
            rig.loops.recovery_writes.push_back(WriteBarrier {
                phase: "held_context",
                entered: entered.clone(),
                release: open.clone(),
            });
            let tx = rig.tx.clone();
            tokio::spawn(async move {
                entered.notified().await;
                control(
                    &tx,
                    serde_json::json!({"type":"thinking", "thinking":false}),
                );
                open.notify_one();
            });
        }
        let prior = if action { 2 } else { 1 };
        let run = gated_final_send(&mut rig, listener, phase, Some(prior), |tx| {
            control(
                tx,
                serde_json::json!({"type":"thinking", "thinking":action}),
            );
            tx.send(code_packet(LATE_EDIT)).unwrap();
        })
        .await;
        assert!(run.flow.is_continue(), "{phase}");
        assert!(old.is_finished() && old.pending_usage_count() == 0);
        let state = &rig.turn.state;
        assert_eq!(state.needs_cold_brief, !action, "{phase}");

        // The briefing that succeeded named the reference; a failed briefing
        // would have left it pending.
        let (pending, named): (&[&str], &[&str]) = if action {
            (&[], &["design"])
        } else {
            (&["design"], &[])
        };
        assert_eq!(state.earlier_steps_pending, pending, "{phase}");
        assert_eq!(state.earlier_steps_named, named, "{phase}");
        assert!(
            state.owed_reply_on_resume.is_some(),
            "an undelivered action cannot settle the debt: {phase}"
        );
        assert_eq!(state.interviewer_silenced(), action, "{phase}");
        assert!(!state.code_shown.contains("late_editor_marker"));

        // Under the newer hold the reply is carried by the resume debt, not by
        // an owed prompt; released, the prompt itself stays owed.
        assert_eq!(rig.turn.activity.owes_prompt(), !action, "{phase}");
        assert_eq!(run.fresh.pending_usage_count(), 1);
        session_usage(&mut rig);
        session_usage(&mut rig);
        assert_eq!(run.fresh.pending_usage_count(), 0);
        let thinking = rig
            .recorder
            .take()
            .into_iter()
            .filter_map(|sent| match sent {
                room_link::Sent::Data { message, .. } if message["type"] == "thinking_state" => {
                    Some(message["thinking"].as_bool().unwrap())
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            thinking.last(),
            Some(&action),
            "the latest Thinking notice survives the failed write: {phase} {thinking:?}"
        );
        let wire = run.finish(rig).await;
        assert_eq!(wire.len(), if action { 2 } else { 1 }, "{phase}: {wire:?}");
        assert_eq!(
            complete_frames(&wire),
            0,
            "{phase}: nothing asked for a reply"
        );
    }
}

// Declared before running. Under a Thinking hold, forty distinct test results
// (the reaction cooldown is reset between them, as elapsed time would) each
// make a held context. The driver retains the newest 32 and counts 8 omitted;
// one drain writes one frame holding the retained summaries in arrival order
// followed by one sentence naming 8 omitted summaries, and then the pending
// count is spent while the running total stays 8. A second batch of 33 omits
// exactly one more: its frame names 1, never 9, and the total is 9.
#[tokio::test]
async fn omitted_held_contexts_are_counted_once_and_reach_the_wire_once() {
    if isolated("omitted_held_contexts_are_counted_once_and_reach_the_wire_once") {
        return;
    }
    let mut rig = Rig::new("ws://127.0.0.1:9/".into()).await;
    rig.turn.state.thinking_hold = crate::agent::ThinkingHold::Held {
        since: Instant::now(),
    };
    let old = rig.gemini.retire();
    let (mut fresh, server) = fake_recording_socket(None, 2).await;
    let boot = crate::runtime::bootstrap(&rig.config, "recovery-test", Some("two-sum"), 45);
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
    {
        let mut context = rig.turn.context(
            &mut rig.output,
            &mut rig.gemini,
            &mut rig.board,
            &mut rig.media,
        );
        let mut driver = Driver::new(&rig.room, &mut context, &mut rig.loops, interview);
        let feed = |driver: &mut Driver<'_, '_, '_>, count: u32| {
            for n in 0..count {
                driver.context.activity.last_test_reaction =
                    Instant::now() - Duration::from_secs(100);
                let packet = test_result_packet(n, &format!("def solve(): return {n}"));
                driver.packet(packet.topic.as_deref(), Some("candidate"), &packet.payload);
            }
        };
        feed(&mut driver, 40);
        assert_eq!(driver.held_contexts.len(), 32);
        assert_eq!((driver.held_omitted, driver.held_omitted_total), (8, 8));
        assert!(driver.pending());
        assert!(!driver.drain(&mut fresh).await.unwrap());
        assert!(driver.held_contexts.is_empty());
        assert_eq!(driver.held_bytes, 0);
        assert_eq!((driver.held_omitted, driver.held_omitted_total), (0, 8));
        assert!(!driver.pending());
        feed(&mut driver, 33);
        assert_eq!((driver.held_omitted, driver.held_omitted_total), (1, 9));
        assert!(!driver.drain(&mut fresh).await.unwrap());
        assert_eq!((driver.held_omitted, driver.held_omitted_total), (0, 9));
        assert!(!driver.pending());
        assert!(driver.context.state.interviewer_silenced());
    }
    let messages = server.await.unwrap();
    assert_eq!(messages.len(), 2);
    let text = |index: usize| {
        messages[index]["clientContent"]["turns"][0]["parts"][0]["text"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    for (index, omitted) in [(0, 8), (1, 1)] {
        assert_eq!(messages[index]["clientContent"]["turnComplete"], false);
        let body = text(index);
        let sentence = format!(
            "[SYSTEM EVENT] {omitted} earlier context summaries exceeded recovery retention"
        );
        assert_eq!(
            body.matches("earlier context summaries exceeded").count(),
            1
        );
        assert!(body.contains(&sentence), "{body}");
        // The newest retained summaries, oldest first, then the sentence.
        let runs = body
            .split("(run #")
            .skip(1)
            .map(|rest| rest.split(',').next().unwrap().parse::<u32>().unwrap())
            .collect::<Vec<_>>();
        let expected = if index == 0 {
            (9..=40).collect::<Vec<u32>>()
        } else {
            (42..=73).collect::<Vec<u32>>()
        };
        assert_eq!(runs, expected, "{index}");
        assert!(
            body.rfind("Latest test run").unwrap()
                < body.find("earlier context summaries").unwrap()
        );
    }
    fresh.shutdown().await.unwrap();
    rig.gemini = old;
    rig.finish().await;
}

// Declared before running. A candidate frame retained from an earlier
// replacement, a never-sent two-byte tail and a turn boundary that arrives
// while the briefing is held must reach the installed socket as: the tail
// joined to the retained frame in one audio write, one audio-end, then the
// first new frame as a separate write. Nothing is dropped or reordered, the
// boundary is not duplicated, and live voice time is not refreshed by the
// retained frame.
#[tokio::test]
async fn retained_audio_boundary_received_during_the_briefing_keeps_wire_order_after_install() {
    if isolated(
        "retained_audio_boundary_received_during_the_briefing_keeps_wire_order_after_install",
    ) {
        return;
    }
    use base64::{Engine, engine::general_purpose::STANDARD};
    let (listener, endpoint) = listener().await;
    let mut rig = Rig::with_old_usage(endpoint, Some(11)).await;
    let arrived = Instant::now() - Duration::from_secs(5);
    rig.media.voiced_at = Some(arrived);
    rig.media.audio_bytes = vec![1, 2];
    rig.media.recovery_audio.push(
        AudioFrame {
            data: vec![7; 80].into(),
            sample_rate: 16000,
            num_channels: 1,
            samples_per_channel: 80,
        },
        arrived,
    );
    let run = gated_final_send(&mut rig, listener, "recovery_brief", None, |tx| {
        control(tx, serde_json::json!({"type":"yield_turn"}));
        control(tx, serde_json::json!({"type":"yield_turn"}));
    })
    .await;
    assert!(run.flow.is_continue());
    assert_eq!(rig.media.audio_bytes, [1, 2], "the tail survives install");
    assert_eq!(rig.media.voiced_at, Some(arrived));
    assert_eq!(rig.media.recovery_audio.dropped_bytes, 0);
    for expected in [Some(true), Some(false), None] {
        let input = tokio::time::timeout(
            Duration::from_millis(200),
            next_candidate_input(&mut rig.media.audio, &mut rig.media.recovery_audio),
        )
        .await;
        match (expected, input) {
            (Some(true), Ok(input @ CandidateInput::BufferedFrame(_)))
            | (Some(false), Ok(input @ CandidateInput::Yield)) => {
                on_candidate_input(
                    &mut rig.turn.context(
                        &mut rig.output,
                        &mut rig.gemini,
                        &mut rig.board,
                        &mut rig.media,
                    ),
                    input,
                    1500,
                )
                .await;
            }

            // With no live track the drained ring ends the stream; nothing else
            // is retained.
            (None, Ok(CandidateInput::End)) => {}
            (expected, got) => panic!(
                "retained input out of order or duplicated: {expected:?} {:?}",
                got.map(|input| match input {
                    CandidateInput::Frame(_) => "frame",
                    CandidateInput::BufferedFrame(_) => "buffered",
                    CandidateInput::Yield => "yield",
                    CandidateInput::End => "end",
                })
            ),
        }
    }
    assert!(rig.media.audio_bytes.is_empty());
    on_candidate_input(
        &mut rig.turn.context(
            &mut rig.output,
            &mut rig.gemini,
            &mut rig.board,
            &mut rig.media,
        ),
        CandidateInput::Frame(AudioFrame {
            data: vec![9; 160].into(),
            sample_rate: 16000,
            num_channels: 1,
            samples_per_channel: 160,
        }),
        1500,
    )
    .await;
    assert_eq!(rig.media.voiced_at, Some(arrived));
    let wire = run.finish(rig).await;
    let audio = wire
        .iter()
        .filter_map(|frame| {
            if frame["realtimeInput"]["audioStreamEnd"] == true {
                Some(None)
            } else {
                frame["realtimeInput"]["audio"]["data"]
                    .as_str()
                    .map(|data| Some(STANDARD.decode(data).unwrap()))
            }
        })
        .collect::<Vec<_>>();
    let mut joined = vec![1, 2];
    joined.extend([7, 0].repeat(80));
    assert_eq!(
        audio,
        [Some(joined), None, Some([9, 0].repeat(160))],
        "tail and retained frame, one boundary, then the new stream"
    );
}

// Declared before running. Thinking is held with a raw debt. A release arrives
// while the first briefing is held and a newer hold arrives while the
// corrected, last briefing is held. The last briefing carried the debt but not
// the newer hold, so it must not settle it: the hold stays, the debt stays
// owed. The corrected briefing's reply request is still the last wire frame (it
// was prepared before the hold), so it is the state, not the wire, that records
// the hold.
#[tokio::test]
async fn the_last_briefing_does_not_settle_a_debt_a_newer_hold_made_current() {
    if isolated("the_last_briefing_does_not_settle_a_debt_a_newer_hold_made_current") {
        return;
    }
    let (listener, endpoint) = listener().await;
    let mut rig = Rig::new(endpoint).await;
    rig.turn.state.thinking_hold = crate::agent::ThinkingHold::Held {
        since: Instant::now(),
    };
    rig.turn.state.owed_reply_on_resume = Some(Some("raw-question-marker".into()));
    rig.turn
        .activity
        .owe_prompt(Instant::now(), Some("raw-question-marker".into()));
    for thinking in [false, true] {
        let (entered, open) = (
            std::sync::Arc::new(tokio::sync::Notify::new()),
            std::sync::Arc::new(tokio::sync::Notify::new()),
        );
        rig.loops.recovery_writes.push_back(WriteBarrier {
            phase: "recovery_brief",
            entered: entered.clone(),
            release: open.clone(),
        });
        let tx = rig.tx.clone();
        tokio::spawn(async move {
            entered.notified().await;
            control(
                &tx,
                serde_json::json!({"type":"thinking", "thinking":thinking}),
            );
            open.notify_one();
        });
    }
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
        socket.next().await.unwrap().unwrap();
        socket
            .send(Message::Text(r#"{"setupComplete":{}}"#.into()))
            .await
            .unwrap();
        let mut messages = Vec::new();
        while let Some(Ok(frame)) = socket.next().await {
            if frame.is_close() {
                break;
            }
            messages
                .push(serde_json::from_str::<serde_json::Value>(frame.to_text().unwrap()).unwrap());
        }
        messages
    });
    assert!(rig.run().await.is_continue());
    assert!(rig.loops.recovery_writes.is_empty());
    assert!(rig.turn.state.interviewer_silenced());
    assert!(
        rig.turn.state.owed_reply_on_resume.is_some(),
        "the last briefing cannot settle a debt the newer hold made current"
    );
    rig.finish().await;
    let messages = server.await.unwrap();
    assert!(messages.len() >= 2, "{messages:?}");
    assert_eq!(
        complete_frames(&messages[messages.len() - 1..]),
        1,
        "the corrected briefing was prepared before the newer hold"
    );
}

// Lifecycle identity of a replacement. Declared before running: every replace()
// logs one `replacement start:` line and exactly one `replacement end:` line
// with the same episode number. A new socket ends `installed`, a setup that was
// tried and refused ends `failed`, a refusal before any connection ends
// `no_setup`, and a terminal intent that preempts the wait ends `cancelled`.
// Milestones keep physical setup apart from the notice, the opener and the
// installation.
fn run_named_replacement_case(name: &str, owe: bool) -> ReplacementRun<'_> {
    run_held_replacement_case(name, owe, None)
}

/// Boxed: a run holds a whole rig, and several of them awaited in one test
/// body would otherwise sit together on the test thread's small stack.
type ReplacementRun<'a> =
    std::pin::Pin<Box<dyn std::future::Future<Output = NamedReplacement> + 'a>>;

/// [`run_named_replacement_case`] with the reconnect notice held for `hold`
/// before it is released, so the time before setup is not setup time.
fn run_held_replacement_case(name: &str, owe: bool, hold: Option<Duration>) -> ReplacementRun<'_> {
    Box::pin(async move {
        let case = replacement_cases()
            .into_iter()
            .find(|case| case.name == name)
            .expect("a declared replacement case");
        let mut rig =
            Rig::with_old_resumption("ws://127.0.0.1:9/".into(), Some(11), case.update.clone())
                .await;
        let config = load_from_pairs([
            ("LIVEKIT_URL", "wss://example.livekit.cloud"),
            ("LIVEKIT_API_KEY", "devkey"),
            ("LIVEKIT_API_SECRET", "devsecret"),
            ("GOOGLE_API_KEYS", case.keys),
        ])
        .unwrap();
        rig.keys = Arc::new(GeminiKeys::from_config(&config));
        rig.config = config;
        let (fresh, endpoint) = listener().await;
        rig.endpoint = endpoint;
        rig.gemini.input_cause = Some("tool");
        let old = rig.gemini.inspect_reader_for_test().unwrap();
        wait_for_received_usage(&old).await;
        rig.loops.restarts = case.restarts;
        if case.outcome != Replaced::Ready {
            rig.turn.state.ended = true;
        }
        rig.turn.activity.latency.anchor(Instant::now());
        if owe {
            rig.turn.activity.latency.prompt(Instant::now());
            rig.turn
                .activity
                .owe_prompt(Instant::now(), Some("raw-owed-marker".into()));
        }
        if let Some(hold) = hold {
            let (entered, release) = rig.recorder.hold_data("interviewer_state", Some(true));
            tokio::spawn(async move {
                entered.notified().await;
                tokio::time::sleep(hold).await;
                release.notify_one();
            });
        }
        let server = tokio::spawn(scripted_fresh_gemini(fresh, case.script));
        let _ = rig.run().await;
        NamedReplacement { rig, server }
    })
}

/// A replacement run whose scripted server still waits for the close that
/// `finish` sends.
struct NamedReplacement {
    rig: Rig,
    server: tokio::task::JoinHandle<(Vec<Accepted>, tokio::net::TcpListener)>,
}

impl NamedReplacement {
    async fn finish(self) {
        self.rig.finish().await;
        let _ = self.server.await.unwrap();
    }
}

fn line_fields(line: &str) -> std::collections::HashMap<&str, &str> {
    let body = &line[..line.rfind(" room=").expect("room is the last field")];
    body.split_whitespace()
        .filter_map(|token| token.split_once('='))
        .collect()
}

#[tokio::test(flavor = "current_thread")]
async fn replacement_episode_lines_resolve_each_start_once_with_its_result() {
    if isolated_checked(
        "replacement_episode_lines_resolve_each_start_once_with_its_result",
        |output| {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let starts = stderr
                .lines()
                .filter(|line| line.starts_with("replacement start: "))
                .map(line_fields)
                .collect::<Vec<_>>();
            let ends = stderr
                .lines()
                .filter(|line| line.starts_with("replacement end: "))
                .map(line_fields)
                .collect::<Vec<_>>();
            assert_eq!(starts.len(), 3, "{stderr}");
            assert_eq!(ends.len(), 3, "{stderr}");
            let results = ["installed", "failed", "no_setup"];
            for ((start, end), result) in starts.iter().zip(&ends).zip(results) {
                assert_eq!(start["episode"], "1");
                assert_eq!(end["episode"], start["episode"]);
                assert_eq!(end["pid"], start["pid"]);
                assert_eq!(end["session"], start["session"]);
                assert_eq!(end["result"], result, "{end:?}");
                assert_eq!(start["trigger"], "close");
                assert_eq!(end["old_socket"], "1");
                assert_eq!(start["advisories"], "0");
                assert_eq!(start["obligation"], "none");
            }

            // Installed: a new socket and replacement number; the physical
            // setup and the notice are separate from the opener and install.
            let done = &ends[0];
            assert_eq!(done["new_socket"], "2");
            assert_eq!(done["replacement"], "1");
            assert_eq!(done["resume_tried"], "true");
            let ms = |key: &str| done[key].parse::<u64>().unwrap();
            assert!(ms("notice_ms") <= ms("connected_ms"), "{done:?}");
            assert!(ms("setup_ms") <= ms("connected_ms"), "{done:?}");
            assert!(ms("connected_ms") <= ms("opener_ms"), "{done:?}");
            assert!(ms("opener_ms") <= ms("install_ms"), "{done:?}");
            // The legacy line keeps its fields and gains the identity.
            let replacement = stderr
                .lines()
                .find(|line| line.starts_with("replacement: "))
                .expect("the installed replacement keeps its legacy line");
            let legacy = line_fields(replacement);
            for key in [
                "at",
                "resumed",
                "trigger",
                "handover",
                "fallback",
                "rotation_gap_ms",
            ] {
                assert!(legacy.contains_key(key), "{replacement}");
            }
            assert_eq!(legacy["episode"], "1");
            assert_eq!(legacy["replacement"], "1");
            assert_eq!(legacy["old_socket"], "1");
            assert_eq!(legacy["new_socket"], "2");
            assert_eq!(legacy["pid"], starts[0]["pid"]);
            // Refusals never installed a socket.
            for end in &ends[1..] {
                assert_eq!(end["new_socket"], "none");
                assert_eq!(end["replacement"], "none");
            }
            assert_eq!(ends[2]["setup_ms"], "none");
            assert_eq!(ends[2]["resume_tried"], "false");
            // Nothing handle- or credential-shaped is logged.
            for line in stderr.lines().filter(|line| {
                line.starts_with("replacement start: ") || line.starts_with("replacement end: ")
            }) {
                assert!(!line.contains("table-handle"), "{line}");
                assert!(!line.contains("sequence-test-key"), "{line}");
            }
        },
    ) {
        return;
    }
    run_named_replacement_case("resume_once_on_the_original_key", false)
        .await
        .finish()
        .await;
    run_named_replacement_case("a_sole_key_billing_failure_is_final", false)
        .await
        .finish()
        .await;
    run_named_replacement_case("spent_charges_refuse_before_any_connection", false)
        .await
        .finish()
        .await;
}

// Declared before running. A reply owed when the socket is replaced keeps its
// obligation number and origin: the episode and the installed replacement name
// that obligation, the trace carries the episode and the later captured audio
// answers the same obligation, with no new origin minted.
#[tokio::test(flavor = "current_thread")]
async fn a_replacement_carries_the_owed_obligation_and_its_origin() {
    if isolated_checked(
        "a_replacement_carries_the_owed_obligation_and_its_origin",
        |output| {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let start = stderr
                .lines()
                .find(|line| line.starts_with("replacement start: "))
                .map(line_fields)
                .expect("start line");
            assert_eq!(start["obligation"], "1");
            assert_eq!(start["obligation_origin"], "prompt");
            let installed = stderr
                .lines()
                .find(|line| line.starts_with("replacement: "))
                .map(line_fields)
                .expect("replacement line");
            assert_eq!(installed["obligation"], "1");
            assert!(!stderr.contains("raw-owed-marker"), "{stderr}");
        },
    ) {
        return;
    }
    let mut run = run_named_replacement_case("resume_once_on_the_original_key", true).await;
    let record = run
        .rig
        .turn
        .activity
        .latency
        .audio(Instant::now() + Duration::from_millis(5))
        .expect("the owed reply is still the open trace");
    assert_eq!(record["obligation"], 1);
    assert_eq!(record["origin"], "prompt");
    assert_eq!(record["obligation_episodes"], serde_json::json!([1]));
    assert_eq!(record["obligation_episode_count"], 1);
    assert_eq!(record["first_accept_socket"], 2);
    assert_eq!(record["turn"], 1, "no second origin was minted");
    run.finish().await;
}

// Declared before running. A terminal intent that preempts the wait ends the
// episode `cancelled`, with no new socket and no replacement number, and the
// start line is not left unresolved.
#[tokio::test]
async fn a_preempted_episode_ends_cancelled_without_a_replacement() {
    if isolated_checked(
        "a_preempted_episode_ends_cancelled_without_a_replacement",
        |output| {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let ends = stderr
                .lines()
                .filter(|line| line.starts_with("replacement end: "))
                .map(line_fields)
                .collect::<Vec<_>>();
            assert_eq!(ends.len(), 1, "{stderr}");
            assert_eq!(ends[0]["result"], "cancelled");
            assert_eq!(ends[0]["episode"], "1");
            assert_eq!(ends[0]["new_socket"], "none");
            assert_eq!(ends[0]["replacement"], "none");
            assert!(
                !stderr.lines().any(|line| line.starts_with("replacement: ")),
                "{stderr}"
            );
        },
    ) {
        return;
    }
    let (listener, endpoint) = listener().await;
    let mut rig = Rig::new(endpoint).await;
    rig.turn.activity.latency.anchor(Instant::now());
    rig.loops.deadline_at = tokio::time::Instant::now() + Duration::from_secs(5);
    let (ready, entered) = oneshot::channel();
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
        socket.next().await.unwrap().unwrap();
        ready.send(()).unwrap();
        closed_tcp(socket.into_inner()).await;
    });
    let stimulus = async {
        entered.await.unwrap();
        tokio::time::pause();
        tokio::time::advance(Duration::from_secs(5)).await;
        tokio::time::resume();
    };
    let (flow, ()) = tokio::join!(rig.run(), stimulus);
    assert!(flow.is_break());
    server.await.unwrap();
    rig.finish().await;
}

// Declared before running. A reconnect notice held for 200 ms delays the
// episode but not the provider's setup: `notice_ms` carries the hold, while
// `setup_ms` stays far below it and `connected_ms` (the legacy
// `rotation_gap_ms`) includes both, so the notice delay is not attributed to
// setup.
#[tokio::test(flavor = "current_thread")]
async fn a_held_reconnect_notice_is_not_attributed_to_provider_setup() {
    if isolated_checked(
        "a_held_reconnect_notice_is_not_attributed_to_provider_setup",
        |output| {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let end = stderr
                .lines()
                .find(|line| line.starts_with("replacement end: "))
                .map(line_fields)
                .expect("end line");
            let ms = |key: &str| end[key].parse::<u64>().unwrap();
            assert!(ms("notice_ms") >= 200, "{end:?}");
            assert!(ms("setup_ms") < 200, "{end:?}");
            assert!(
                ms("connected_ms") >= ms("notice_ms") + ms("setup_ms"),
                "{end:?}"
            );
            let legacy = stderr
                .lines()
                .find(|line| line.starts_with("replacement: "))
                .map(line_fields)
                .expect("replacement line");
            assert_eq!(
                legacy["rotation_gap_ms"].parse::<u64>().unwrap(),
                ms("connected_ms"),
                "the legacy gap is the connected milestone, notice included"
            );
        },
    ) {
        return;
    }
    run_held_replacement_case(
        "resume_once_on_the_original_key",
        false,
        Some(Duration::from_millis(200)),
    )
    .await
    .finish()
    .await;
}

// Declared before running. A child that logs more than a pipe holds must finish
// and have all of its output checked: the parent drains both pipes while it
// waits, so a chatty child is not mistaken for a hung one.
#[test]
fn an_isolated_child_may_log_more_than_a_pipe_holds() {
    if isolated_checked(
        "an_isolated_child_may_log_more_than_a_pipe_holds",
        |output| {
            assert!(output.stderr.len() > 100_000, "{}", output.stderr.len());
            let tail = String::from_utf8_lossy(&output.stderr);
            assert!(
                tail.trim_end().ends_with("line 999"),
                "the last line survives"
            );
        },
    ) {
        return;
    }
    for n in 0..1000 {
        eprintln!("{:->120} line {n}", "");
    }
}

// Socket pressure summaries. Declared before running: one replacement of the
// first socket and the exit of the second leave exactly two `socket_pressure`
// records on the latency channel, socket 1 ended `replacement` and socket 2
// `terminal`, each logged once however many times the terminal drain runs. They
// carry counters and identity only: the key set is exactly the documented one,
// and no handle, credential, or owed-prompt text appears anywhere.
#[tokio::test(flavor = "current_thread")]
async fn each_released_socket_logs_one_bounded_pressure_summary() {
    if isolated_checked(
        "each_released_socket_logs_one_bounded_pressure_summary",
        |output| {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let records = stderr
                .lines()
                .filter_map(|line| line.strip_prefix("codetrial latency "))
                .map(|json| serde_json::from_str::<serde_json::Value>(json).unwrap())
                .filter(|record| record["event"] == "socket_pressure")
                .collect::<Vec<_>>();
            assert_eq!(records.len(), 2, "{stderr}");
            let keys = [
                "blocked_ms_max",
                "blocked_ms_total",
                "blocked_sends",
                "capacity",
                "consumed",
                "end",
                "event",
                "interview_ms",
                "occupancy_high_water",
                "pid",
                "reader_end",
                "received",
                "room",
                "session",
                "socket",
                "version",
            ];
            for record in &records {
                let mut found = record
                    .as_object()
                    .unwrap()
                    .keys()
                    .map(String::as_str)
                    .collect::<Vec<_>>();
                found.sort_unstable();
                assert_eq!(found, keys, "{record}");
                assert_eq!(record["capacity"], 256);
                assert_eq!(record["blocked_sends"], 0);
                assert_eq!(record["blocked_ms_total"], 0);
                assert_eq!(record["blocked_ms_max"], 0);
                assert_eq!(record["session"], records[0]["session"]);
                assert_eq!(record["pid"], records[0]["pid"]);
                assert_eq!(record["room"], "recovery-test");
            }
            assert_eq!(records[0]["socket"], 1);
            assert_eq!(records[0]["end"], "replacement");
            assert_eq!(records[0]["received"], 1, "the one usage frame");
            assert_eq!(records[1]["socket"], 2);
            assert_eq!(records[1]["end"], "terminal");
            assert_eq!(records[1]["reader_end"], "stopped");
            for record in &records {
                assert!(record["consumed"].as_u64() <= record["received"].as_u64());
            }
            for secret in ["raw-owed-marker", "table-handle", "sequence-test-key"] {
                assert!(
                    !stderr
                        .lines()
                        .filter(|line| line.contains("socket_pressure"))
                        .any(|line| line.contains(secret)),
                    "{secret}"
                );
            }
        },
    ) {
        return;
    }
    let mut run = run_named_replacement_case("resume_once_on_the_original_key", true).await;
    run.rig.gemini.shutdown().await.unwrap();
    for _ in 0..2 {
        let rig = &mut run.rig;
        session::drain_live_usage(
            &rig.room,
            &mut rig.turn.context(
                &mut rig.output,
                &mut rig.gemini,
                &mut rig.board,
                &mut rig.media,
            ),
        );
    }
    run.finish().await;
}

// Mixed frames queued behind a held consumer, then a rotation or a stop.
//
// The old socket's peer sends, in one burst: audio, an output transcript, a
// tool call, a usage observation (21), a resumable checkpoint "h1", more audio,
// usage 22 together with the turn completion, a newer checkpoint "h2", and a
// non-resumable update. The consumer is held, so the reader has applied every
// frame's bookkeeping (handle, ledger) while the room has taken only the first
// k of the 8 queued events (a closing usage-only frame ends the burst, so that
// reading it proves the frames before it were applied). Then the actual
// replacement runs, entered with the GoAway and the Watchdog trigger: the
// socket is replaced while events are still queued, which the close trigger (a
// drained channel) could not do.
//
// Declared before running, for every k in {0, 3, 4, 8}:
//
// * counters at replace time: 8 received, k consumed, the handle "h2", and
//   3 - (usage events among the first k) observations still on the ledger;
// * the replacement resumes from "h2" however much was consumed: a handle newer
//   than the consumed events is not itself a defect, and is not reported as
//   one; the fresh socket's setup carries "h2" and never the non-resumable one;
// * no stale output: the room delivered 2 output frames per audio event it
//   consumed before the replacement (0, 2, 2, 4) and the replacement
//   delivers none of the audio the old queue still held;
// * a queued tool call is never executed late: `volunteered_hints` is 1 only
//   when the call was among the first k events (k >= 3), the old socket
//   receives its response exactly then, and the fresh socket receives none;
// * each usage observation is recorded exactly once whatever k is (consumed
//   ones when handled, the rest when the old socket is let go): the
//   `live_turn_usage` lines of a row are prompts 21, 22, 23 (a third
//   usage-only frame closes the burst);
// * the pressure summary of the old socket says 8 received, k consumed, ended
//   at the replacement, so the unconsumed remainder is visible;
// * the reply stays owed unless the room consumed the turn completion (event
//   6, so k = 8 here): the briefing asks for it once when owed and not at all
//   when not (an unconsumed tail is never counted as delivered);
//
// Limits: scripted frames over loopback; the speech-prefix variant is covered
// by its own tests; nothing here says what the provider does with a handle
// newer than what the room consumed, nor what it holds for a tool call the room
// never saw, and the briefing text about such a call is not asserted.

fn held_mixed_frames() -> Vec<String> {
    // 960 bytes: two 10 ms frames of 24 kHz mono PCM, so a delivery is visible.
    let data = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, [0u8; 960]);
    let audio = || {
        format!(
            r#"{{"serverContent":{{"modelTurn":{{"parts":[{{"inlineData":{{"mimeType":"audio/pcm","data":"{data}"}}}}]}}}}}}"#
        )
    };
    vec![
        audio(),
        r#"{"serverContent":{"outputTranscription":{"text":"so tell me"}}}"#.into(),
        format!(
            r#"{{"toolCall":{{"functionCalls":[{{"id":"call-q","name":"{}","args":{{"requested":false}}}}]}}}}"#,
            crate::runtime::TOOL_LOG_HINT
        ),
        r#"{"usageMetadata":{"promptTokenCount":21}}"#.into(),
        r#"{"sessionResumptionUpdate":{"resumable":true,"newHandle":"h1"}}"#.into(),
        audio(),
        r#"{"serverContent":{"turnComplete":true},"usageMetadata":{"promptTokenCount":22}}"#.into(),
        r#"{"sessionResumptionUpdate":{"resumable":true,"newHandle":"h2"}}"#.into(),
        r#"{"sessionResumptionUpdate":{"resumable":false,"newHandle":"not-resumable"}}"#.into(),
        r#"{"usageMetadata":{"promptTokenCount":23}}"#.into(),
    ]
}

impl Rig {
    /// A rig whose live socket has queued [`held_mixed_frames`] that nothing
    /// has consumed yet, and an owed reply.
    async fn with_held_mixed_frames() -> Self {
        let mut rig = Self::new("ws://127.0.0.1:9/".into()).await;
        rig.old.take().unwrap().abort();
        let (listener, endpoint) = listener().await;
        rig.old = Some(tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
            socket.next().await.unwrap().unwrap();
            socket
                .send(Message::Text(r#"{"setupComplete":{}}"#.into()))
                .await
                .unwrap();
            for frame in held_mixed_frames() {
                socket.send(Message::Text(frame.into())).await.unwrap();
            }
            let mut messages = Vec::new();
            while let Some(Ok(frame)) = socket.next().await {
                if frame.is_close() {
                    break;
                }
                messages.push(serde_json::from_str(frame.to_text().unwrap()).unwrap());
            }
            messages
        }));
        let boot = crate::runtime::bootstrap(&rig.config, "recovery-test", Some("two-sum"), 45);
        rig.gemini = crate::gemini::live_session_with_keys_at(&endpoint, &rig.keys, &boot, None)
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            while rig.gemini.pressure_for_test().received < 8 {
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .expect("the reader applies every frame while the room is held");
        assert_eq!(
            rig.gemini.resumption_handle().as_deref(),
            Some("h2"),
            "the closing frame came after the non-resumable update"
        );
        rig.turn.activity.latency.anchor(Instant::now());
        rig.turn.activity.latency.prompt(Instant::now());
        rig.turn
            .activity
            .owe_prompt(Instant::now(), Some("raw-owed-marker".into()));
        rig
    }

    /// Hands the first `count` queued events to the room, as its loop would,
    /// and returns how many output frames the room delivered for them.
    async fn consume_queued(&mut self, count: usize) -> usize {
        let mut delivered = 0;
        for _ in 0..count {
            let event = self.gemini.next_event().await.unwrap();
            let mut context = self.turn.context(
                &mut self.output,
                &mut self.gemini,
                &mut self.board,
                &mut self.media,
            );
            session::handle_gemini_event(&self.room, &mut context, event, Interruptible::Yes)
                .await
                .unwrap();
            delivered += self.delivered_frames();
        }
        delivered
    }

    /// The actual replacement, entered by `trigger` with events still queued.
    async fn replace_with(&mut self, trigger: Trigger) -> ControlFlow<()> {
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
        tokio::time::timeout(Duration::from_secs(10), async {
            let mut context = self.turn.context(
                &mut self.output,
                &mut self.gemini,
                &mut self.board,
                &mut self.media,
            );
            replace_gemini_session(
                &self.room,
                &mut context,
                interview,
                &mut self.loops,
                trigger,
                None,
            )
            .await
        })
        .await
        .expect("the real room recovery must be bounded in the test")
        .unwrap()
    }

    fn delivered_frames(&mut self) -> usize {
        let mut frames = 0;
        while self._frames.try_recv().is_ok() {
            frames += 1;
        }
        frames
    }
}

fn prompts_of(lines: &[String]) -> Vec<String> {
    lines
        .iter()
        .map(|line| {
            line.split_whitespace()
                .find_map(|field| field.strip_prefix("prompt_tokens="))
                .unwrap_or("?")
                .to_owned()
        })
        .collect()
}

#[tokio::test(flavor = "current_thread")]
async fn queued_mixed_frames_survive_a_rotation_with_exact_received_and_consumed_state() {
    const CUTS: [usize; 4] = [0, 3, 4, 8];
    const TRIGGERS: [Trigger; 2] = [Trigger::GoAway, Trigger::Watchdog];
    let rows = CUTS.len() * TRIGGERS.len();
    if isolated_checked(
        "queued_mixed_frames_survive_a_rotation_with_exact_received_and_consumed_state",
        |output| {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let usage = usage_lines(output);
            assert_eq!(usage.len(), 3 * rows, "{stderr}");
            assert_eq!(
                prompts_of(&usage),
                (0..rows)
                    .flat_map(|_| ["21".to_owned(), "22".to_owned(), "23".to_owned()])
                    .collect::<Vec<_>>(),
                "each observation exactly once per row: {stderr}"
            );
            let pressure = stderr
                .lines()
                .filter_map(|line| line.strip_prefix("codetrial latency "))
                .map(|json| serde_json::from_str::<serde_json::Value>(json).unwrap())
                .filter(|record| record["event"] == "socket_pressure" && record["socket"] == 1)
                .collect::<Vec<_>>();
            assert_eq!(pressure.len(), rows, "{stderr}");
            for (record, cut) in pressure.iter().zip(CUTS.iter().flat_map(|cut| [cut; 2])) {
                assert_eq!(record["received"], 8, "{record}");
                assert_eq!(record["consumed"], *cut, "{record}");
                assert_eq!(record["end"], "replacement", "{record}");
                assert_eq!(record["blocked_sends"], 0, "{record}");
            }
            assert!(
                !stderr
                    .split(|c: char| !c.is_ascii_alphanumeric())
                    .any(|token| token == "h1" || token == "h2" || token == "newHandle"),
                "a handle must not be logged: {stderr}"
            );
            assert!(!stderr.contains("raw-owed-marker"), "{stderr}");
        },
    ) {
        return;
    }
    for (cut, trigger) in CUTS
        .into_iter()
        .flat_map(|cut| TRIGGERS.map(|trigger| (cut, trigger)))
    {
        let label = format!("cut={cut} {trigger:?}");
        let (fresh, endpoint) = listener().await;
        let mut rig = Rig::with_held_mixed_frames().await;
        rig.endpoint = endpoint;
        let delivered_before = rig.consume_queued(cut).await;
        let at_replace = rig.gemini.pressure_for_test();
        assert_eq!((at_replace.received, at_replace.consumed), (8, cut as u64));
        assert!(rig.gemini.checkpoint_age().is_some(), "{label}");
        assert_eq!(rig.gemini.resumption_handle().as_deref(), Some("h2"));
        let usage_events_consumed = [3usize, 5, 7].iter().filter(|at| **at < cut).count();
        assert_eq!(
            rig.gemini
                .inspect_reader_for_test()
                .unwrap()
                .pending_usage_count(),
            3 - usage_events_consumed,
            "{label}"
        );
        assert_eq!(
            rig.turn.state.volunteered_hints,
            u32::from(cut >= 3),
            "{label}"
        );
        assert_eq!(
            delivered_before,
            2 * usize::from(cut >= 1) + 2 * usize::from(cut >= 5),
            "{label}: audio the room consumed reaches the output"
        );
        let server = tokio::spawn(recording_fresh_gemini(fresh, vec![FreshStep::Ready]));

        let flow = rig.replace_with(trigger).await;
        assert!(flow.is_continue(), "{label}");
        assert_eq!(rig.loops.restarts, 1, "{label}");
        assert_eq!(rig.turn.activity.live_socket, 2, "{label}");
        assert_eq!(
            rig.delivered_frames(),
            0,
            "{label}: no output after the rotation (before it: {delivered_before})"
        );
        assert_eq!(
            rig.turn.state.volunteered_hints,
            u32::from(cut >= 3),
            "{label}: a queued call is not executed late"
        );
        assert_eq!(
            rig.turn.activity.owes_prompt(),
            cut < 7,
            "{label}: owed unless the whole reply was consumed"
        );
        let old = rig.old_frames().await;
        assert_eq!(
            tool_response_ids(&old),
            if cut >= 3 {
                vec!["call-q".to_string()]
            } else {
                Vec::new()
            },
            "{label}"
        );
        let seen = server.await.unwrap();
        assert_eq!(seen.len(), 1, "{label}");
        let (handle, frames) = &seen[0];
        assert_eq!(handle.as_deref(), Some("h2"), "{label}");
        assert!(tool_response_ids(frames).is_empty(), "{label}");
        let turns = briefing_turns(frames);
        assert_eq!(
            turns.iter().filter(|asked| **asked).count(),
            usize::from(cut < 7),
            "{label}: {turns:?}"
        );
    }
}

// The same sequence with the terminal arriving while the resume open is in
// flight. Declared before running, per row k in {0, 3, 4, 8}: the stop breaks
// the room loop, the usage observations were already recorded exactly once
// (consumed ones when handled, the rest when the old socket was let go before
// the open began), the old socket's pressure summary is 8 received and k
// consumed and ends at the replacement, no output reaches the candidate after
// the stop, the reply is still owed unless the turn completion was consumed (k
// = 8), no socket is installed, and no further open is attempted.
#[tokio::test(flavor = "current_thread")]
async fn queued_mixed_frames_are_accounted_once_when_a_stop_cancels_the_resume_open() {
    const CUTS: [usize; 4] = [0, 3, 4, 8];
    if isolated_checked(
        "queued_mixed_frames_are_accounted_once_when_a_stop_cancels_the_resume_open",
        |output| {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let usage = usage_lines(output);
            assert_eq!(
                prompts_of(&usage),
                CUTS.iter()
                    .flat_map(|_| ["21".to_owned(), "22".to_owned(), "23".to_owned()])
                    .collect::<Vec<_>>(),
                "{stderr}"
            );
            let consumed = stderr
                .lines()
                .filter_map(|line| line.strip_prefix("codetrial latency "))
                .map(|json| serde_json::from_str::<serde_json::Value>(json).unwrap())
                .filter(|record| record["event"] == "socket_pressure" && record["socket"] == 1)
                .map(|record| {
                    assert_eq!(record["end"], "replacement", "{record}");
                    (record["received"].clone(), record["consumed"].clone())
                })
                .collect::<Vec<_>>();
            assert_eq!(
                consumed,
                CUTS.iter()
                    .map(|cut| (serde_json::json!(8), serde_json::json!(cut)))
                    .collect::<Vec<_>>(),
                "{stderr}"
            );
        },
    ) {
        return;
    }
    for cut in CUTS {
        let label = format!("cut={cut}");
        let (fresh, endpoint) = listener().await;
        let mut rig = Rig::with_held_mixed_frames().await;
        rig.endpoint = endpoint;
        let delivered_before = rig.consume_queued(cut).await;
        assert_eq!(
            delivered_before,
            2 * usize::from(cut >= 1) + 2 * usize::from(cut >= 5),
            "{label}"
        );
        rig.loops.deadline_at = tokio::time::Instant::now() + Duration::from_secs(5);
        let deadline = rig.loops.deadline_at;
        let (probe, mut observed) = mpsc::unbounded_channel();
        rig.loops.recovery_terminal = Some(probe);
        let (unused, _) = mpsc::unbounded_channel();
        let mut room = Some(std::mem::replace(&mut rig._room_sender, unused));
        let presence_now = std::sync::Arc::new(std::sync::Mutex::new(Instant::now()));
        let (entered, pending) = oneshot::channel();
        let server = tokio::spawn(async move {
            use tokio::io::AsyncReadExt;
            let (mut socket, _) = fresh.accept().await.unwrap();
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") {
                assert!(request.len() < 4096);
                request.push(socket.read_u8().await.unwrap());
            }
            entered.send(()).unwrap();
            closed_tcp(socket).await;
            fresh
        });
        let tx = rig.tx.clone();
        let stimulus = async {
            pending.await.unwrap();
            stop_pending_open(
                OpenStop::End,
                deadline,
                &tx,
                &mut room,
                &mut observed,
                &presence_now,
            )
            .await;
        };
        let (flow, ()) = tokio::join!(rig.replace_with(Trigger::GoAway), stimulus);
        assert!(flow.is_break(), "{label}");
        assert_eq!(rig.loops.restarts, 1, "{label}");
        assert_eq!(rig.turn.activity.live_socket, 1, "{label}");
        assert_eq!(
            rig.turn.activity.owes_prompt(),
            cut < 7,
            "{label}: owed unless the whole reply was consumed"
        );
        assert_eq!(
            rig.delivered_frames(),
            0,
            "{label}: no output after the stop (before it: {delivered_before})"
        );
        assert_eq!(
            rig.turn.state.volunteered_hints,
            u32::from(cut >= 3),
            "{label}"
        );
        assert!(observed.try_recv().is_err(), "{label}: one terminal");
        let old = rig.old_frames().await;
        assert_eq!(
            tool_response_ids(&old),
            if cut >= 3 {
                vec!["call-q".to_string()]
            } else {
                Vec::new()
            },
            "{label}"
        );
        let listener = server.await.unwrap();
        assert_eq!(
            listener.into_std().unwrap().accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock,
            "{label}: no further open"
        );
    }
}

// Candidate-audio stores, characterized separately (T5b.1). The SDK queue is in
// `media.rs` tests; this block covers the owned ingress, the attempted versus
// never-attempted tails, and the boundaries between them.

fn numbered_frame(marker: i16, samples: usize) -> AudioFrame<'static> {
    AudioFrame {
        data: vec![marker; samples].into(),
        sample_rate: 16000,
        num_channels: 1,
        samples_per_channel: samples as u32,
    }
}

/// Ten millisecond frames arrive for a stall of 20, 39, 40, 41 and 60 s of
/// virtual time with nothing leaving the ingress, then the consumer returns.
/// The frame cap, the byte cap and the 40 s age all meet at 4000 frames, so
/// the row at 40 s is the boundary and the later rows show which store drops
/// first. At this cadence the age expiry reaches each frame no later than the
/// cap would, so the age is what drops frames; the cap alone is pinned by the
/// same-instant row of `ingress_has_independent_frame_byte_and_age_bounds`.
/// The age compare is `>=`, so the frame exactly 40 s old is already gone.
#[test]
fn ingress_stall_markers_at_20_39_40_41_and_60_seconds() {
    // (stall seconds, delivered, first delivered marker, frames dropped)
    let rows = [
        (20u64, 2000usize, 1i16, 0usize),
        (39, 3900, 1, 0),
        (40, 3999, 2, 1),
        (41, 3999, 102, 101),
        (60, 3999, 2002, 2001),
    ];
    for (stall, delivered, first, dropped) in rows {
        let start = Instant::now();
        let mut ring = Ingress::default();
        let arrivals = i16::try_from(stall * 100).unwrap();
        for marker in 1..=arrivals {
            let at = start + Duration::from_millis(u64::try_from(marker - 1).unwrap() * 10);
            ring.push(numbered_frame(marker, 160), at);
            assert!(ring.frames.len() <= INGRESS_FRAMES, "stall {stall}s");
            assert!(ring.bytes <= INGRESS_BYTES, "stall {stall}s");
        }
        let read_at = start + Duration::from_secs(stall);
        let mut markers = Vec::new();
        while let Some(input) = ring.pop(read_at) {
            let CandidateInput::BufferedFrame(frame) = input else {
                panic!("only audio was pushed, stall {stall}s");
            };
            markers.push(frame.data[0]);
        }
        assert_eq!(markers.len(), delivered, "stall {stall}s");
        assert_eq!(markers.first().copied(), Some(first), "stall {stall}s");
        assert!(
            markers.windows(2).all(|pair| pair[1] == pair[0] + 1),
            "survivors are contiguous and ordered, stall {stall}s"
        );
        assert_eq!(markers.last().copied(), Some(arrivals), "stall {stall}s");
        assert_eq!(ring.dropped_bytes, dropped * 320, "stall {stall}s");
        assert_eq!(ring.bytes, 0);

        // The oldest delivered frame was held under 40 s locally. That is all
        // this store knows: it never saw the frame before it was pushed.
        let oldest_dwell =
            read_at - (start + Duration::from_millis(u64::try_from(first - 1).unwrap() * 10));
        assert!(oldest_dwell < INGRESS_AGE, "stall {stall}s");
    }
}

/// A held reply is deferred only by audio still inside the age bound, and a
/// republish adds one boundary after whole retained frames but none for an
/// ingress that is already empty, nor for the never-attempted tail alone.
#[test]
fn republish_eof_and_hold_boundaries_follow_the_age_bound() {
    // Hold: audio 30 s old defers a reply, audio 41 s old does not and is
    // counted as dropped by the read that finds it expired.
    for (age, defers) in [(30u64, true), (41, false)] {
        let mut ring = Ingress::default();
        ring.push(
            numbered_frame(3, 160),
            Instant::now() - Duration::from_secs(age),
        );
        assert_eq!(ring.has_audio(), defers, "age {age}s");
        assert_eq!(ring.dropped_bytes, if defers { 0 } else { 320 });
    }

    // Republish with nothing owned: the tail alone neither invents an audio-end
    // boundary nor leaves ingress non-empty.
    let mut media = CandidateMedia::new();
    media.audio_bytes = vec![1, 2];
    media.begin_audio_publication();
    assert_eq!(media.audio_bytes, [1, 2]);
    assert_eq!(media.quarantined_audio_bytes, 0);
    assert!(media.recovery_audio.is_empty());

    // Republish over only expired frames: the frames are dropped and counted,
    // and the single boundary that remains is still delivered once.
    let mut media = CandidateMedia::new();
    media.recovery_audio.push(
        numbered_frame(5, 160),
        Instant::now() - INGRESS_AGE - Duration::from_secs(1),
    );
    media.begin_audio_publication();
    assert_eq!(media.recovery_audio.dropped_bytes, 320);
    let now = Instant::now();
    assert!(matches!(
        media.recovery_audio.pop(now),
        Some(CandidateInput::Yield)
    ));
    assert!(media.recovery_audio.pop(now).is_none());

    // Republish over live frames with an ambiguous tail: the tail is counted as
    // quarantined, the frames survive in order, and a second publication does
    // not add a second boundary.
    let mut media = CandidateMedia::new();
    media.audio_bytes = vec![9, 9, 9, 9];
    media.audio_unconfirmed = true;
    let now = Instant::now();
    for marker in [1, 2] {
        media.recovery_audio.push(numbered_frame(marker, 80), now);
    }
    media.begin_audio_publication();
    media.begin_audio_publication();
    assert_eq!(media.quarantined_audio_bytes, 4);
    assert!(media.audio_bytes.is_empty());
    let mut order = Vec::new();
    while let Some(input) = media.recovery_audio.pop(now) {
        order.push(match input {
            CandidateInput::BufferedFrame(frame) => i32::from(frame.data[0]),
            CandidateInput::Yield => -1,
            _ => unreachable!("only buffered audio and a boundary were pushed"),
        });
    }
    assert_eq!(order, [1, 2, -1]);
    assert_eq!(media.recovery_audio.dropped_bytes, 0);
}

#[derive(Debug, PartialEq)]
enum Wire {
    Audio(Vec<u8>),
    AudioEnd,
}

/// What reaches the provider after a replacement commits, in order, with an
/// expired frame, a never-attempted or ambiguous tail, live frames and one
/// boundary. Each sample is numbered so a replay, a reorder or a second
/// audio-end shows up as a different sequence, not only a different count.
/// The drops are counted locally and the log line says so.
#[tokio::test]
async fn recovery_wire_orders_the_unsent_tail_before_live_ingress_and_counts_every_drop() {
    if isolated_checked(
        "recovery_wire_orders_the_unsent_tail_before_live_ingress_and_counts_every_drop",
        |output| {
            let stderr = String::from_utf8_lossy(&output.stderr);
            for expected in [
                "recovery ingress: dropped_bytes=320 unconfirmed_bytes=0 unsent_bytes=2 ",
                "recovery ingress: dropped_bytes=320 unconfirmed_bytes=2 unsent_bytes=0 ",
            ] {
                assert!(stderr.contains(expected), "missing {expected:?}:\n{stderr}");
            }
        },
    ) {
        return;
    }
    use base64::{Engine, engine::general_purpose::STANDARD};
    for uncertain in [false, true] {
        let (listener, endpoint) = listener().await;
        let mut rig = Rig::new(endpoint).await;
        rig.media.audio_bytes = vec![1, 2];
        rig.media.audio_unconfirmed = uncertain;
        let now = Instant::now();
        // Older than the age bound: dropped and counted, never on the wire.
        rig.media.recovery_audio.push(
            numbered_frame(5, 160),
            now - INGRESS_AGE - Duration::from_secs(1),
        );
        rig.media.recovery_audio.push(numbered_frame(7, 80), now);
        rig.media.recovery_audio.yielded(now);
        rig.media.recovery_audio.push(numbered_frame(9, 160), now);
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
            socket.next().await.unwrap().unwrap();
            socket
                .send(Message::Text(r#"{"setupComplete":{}}"#.into()))
                .await
                .unwrap();
            for _ in 0..2 {
                socket.next().await.unwrap().unwrap();
            }
            let mut wire = Vec::new();
            loop {
                let frame = socket.next().await.unwrap().unwrap();
                if frame.is_close() {
                    break;
                }
                let value: serde_json::Value =
                    serde_json::from_str(frame.to_text().unwrap()).unwrap();
                if value["realtimeInput"]["audioStreamEnd"] == true {
                    wire.push(Wire::AudioEnd);
                } else {
                    wire.push(Wire::Audio(
                        STANDARD
                            .decode(value["realtimeInput"]["audio"]["data"].as_str().unwrap())
                            .unwrap(),
                    ));
                }
            }
            wire
        });
        assert!(rig.run().await.is_continue());
        assert_eq!(
            rig.media.audio_bytes,
            if uncertain { vec![] } else { vec![1, 2] }
        );
        for _ in 0..3 {
            let input =
                next_candidate_input(&mut rig.media.audio, &mut rig.media.recovery_audio).await;
            on_candidate_input(
                &mut rig.turn.context(
                    &mut rig.output,
                    &mut rig.gemini,
                    &mut rig.board,
                    &mut rig.media,
                ),
                input,
                1500,
            )
            .await;
        }
        assert!(rig.media.recovery_audio.is_empty());
        assert_eq!(
            rig.media.recovery_audio.dropped_bytes, 320,
            "the cumulative count survives the commit"
        );
        rig.finish().await;
        let mut first = if uncertain { Vec::new() } else { vec![1, 2] };
        first.extend([7, 0].repeat(80));
        assert_eq!(
            server.await.unwrap(),
            [
                Wire::Audio(first),
                Wire::AudioEnd,
                Wire::Audio([9, 0].repeat(160)),
            ],
            "uncertain={uncertain}: one boundary, no replayed tail, nothing expired"
        );
    }
}

/// A terminal intent and an audio frame ready in the same poll: the biased
/// select takes the intent, so the frame stays in the SDK store instead of
/// being moved into an ingress of a recovery that is being cancelled.
#[tokio::test]
async fn same_poll_terminal_wins_and_leaves_the_sdk_frame_unread() {
    if isolated("same_poll_terminal_wins_and_leaves_the_sdk_frame_unread") {
        return;
    }
    use ::livekit::webrtc::audio_source::AudioSourceOptions;
    use ::livekit::webrtc::audio_source::native::NativeAudioSource;
    use ::livekit::webrtc::audio_stream::native::{NativeAudioStream, NativeAudioStreamOptions};
    let mut rig = Rig::new("ws://127.0.0.1:9/".into()).await;
    let source = NativeAudioSource::new(AudioSourceOptions::default(), 16_000, 1, 0);
    let track = ::livekit::prelude::LocalAudioTrack::create_audio_track(
        "same-poll",
        ::livekit::prelude::RtcAudioSource::Native(source.clone()),
    );
    rig.media.audio = Some(NativeAudioStream::with_options(
        track.rtc_track(),
        16_000,
        1,
        NativeAudioStreamOptions {
            queue_size_frames: Some(4000),
        },
    ));
    source.capture_frame(&numbered_frame(7, 160)).await.unwrap();

    // Capture delivers on this thread; the pause only removes any doubt that
    // the frame is queued before the first poll, so the terminal cannot win
    // merely because the frame had not arrived.
    tokio::time::sleep(Duration::from_millis(100)).await;
    control(&rig.tx, serde_json::json!({"type":"end_interview"}));
    let boot = crate::runtime::bootstrap(&rig.config, "recovery-test", Some("two-sum"), 45);
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
    {
        let mut context = rig.turn.context(
            &mut rig.output,
            &mut rig.gemini,
            &mut rig.board,
            &mut rig.media,
        );
        let mut driver = Driver::new(&rig.room, &mut context, &mut rig.loops, interview);
        assert!(driver.wait(std::future::pending::<()>()).await.is_err());
        assert!(matches!(driver.terminal, Some(Terminal::End { .. })));
        assert!(driver.ingress.is_empty(), "the frame was not moved");
        assert_eq!(driver.ingress.dropped_bytes, 0);
    }
    let frame = next_audio_frame(&mut rig.media.audio)
        .await
        .expect("the SDK store still holds the frame");
    assert_eq!(frame.data[0], 7);
    rig.finish().await;
}

/// The ingress stamps a frame when the driver moves it out of the SDK queue,
/// not when it was captured: a frame that waited in the SDK store while the
/// loop was away starts a fresh 40 s local dwell, so the stores add up.
#[tokio::test]
async fn ingress_stamps_sdk_frames_when_drained_not_when_captured() {
    if isolated("ingress_stamps_sdk_frames_when_drained_not_when_captured") {
        return;
    }
    use ::livekit::webrtc::audio_source::AudioSourceOptions;
    use ::livekit::webrtc::audio_source::native::NativeAudioSource;
    use ::livekit::webrtc::audio_stream::native::{NativeAudioStream, NativeAudioStreamOptions};
    let mut rig = Rig::new("ws://127.0.0.1:9/".into()).await;
    let source = NativeAudioSource::new(AudioSourceOptions::default(), 16_000, 1, 0);
    let track = ::livekit::prelude::LocalAudioTrack::create_audio_track(
        "stamp-on-drain",
        ::livekit::prelude::RtcAudioSource::Native(source.clone()),
    );
    rig.media.audio = Some(NativeAudioStream::with_options(
        track.rtc_track(),
        16_000,
        1,
        NativeAudioStreamOptions {
            queue_size_frames: Some(4000),
        },
    ));
    let captured_at = Instant::now();
    source.capture_frame(&numbered_frame(7, 160)).await.unwrap();
    // The room loop is elsewhere for a while; the frame waits in the SDK.
    tokio::time::sleep(Duration::from_millis(300)).await;
    let boot = crate::runtime::bootstrap(&rig.config, "recovery-test", Some("two-sum"), 45);
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
    {
        let mut context = rig.turn.context(
            &mut rig.output,
            &mut rig.gemini,
            &mut rig.board,
            &mut rig.media,
        );
        let mut driver = Driver::new(&rig.room, &mut context, &mut rig.loops, interview);
        driver
            .wait(tokio::time::sleep(Duration::from_millis(50)))
            .await
            .unwrap();
        assert_eq!(driver.ingress.frames.len(), 1);
        let (stamped, _) = &driver.ingress.frames[0];
        assert!(
            stamped.duration_since(captured_at) >= Duration::from_millis(300),
            "stamped when drained, so the SDK wait is not part of the local age"
        );
    }
    rig.finish().await;
}

// Provider-outage schedules in logical time (policy decision record).
//
// Every row drives the real `retry_cold_open_into` loop, with its real budget,
// backoff and quota-wait arms, against a fake provider on a paused clock, so a
// 420-second outage takes no wall time. Only the provider is modelled:
//
// * `FastRefusal`: a failed open is a refusal answered after 100 ms.
// * `SlowFailure`: a failed open spends the 30-second connect-plus-setup limit.
// * `QuotaGrid`: a failed contact leaves every key cooling for 60 seconds after
//   the answer, and the loop waits for the first key back (the exhausted
//   rotation arm). `extension` models another interview re-cooling the shared
//   keys: from `at` seconds an open that finds the keys eligible instead finds
//   them cooling until `until`.
//
// A successful open is answered after 100 ms, so an open that starts at the
// edge of a limit never ties with it. The loop starts with one charge spent, as
// `replace` spends the initial charge before it reaches the cold retry. The
// "candidate" is the loop bounded by `timeout(limit)`, which is what an episode
// deadline measured from the first replacement intent amounts to; the
// "terminal" bound is the room control that `Driver::wait` honours earlier.
//
// Expected values below were declared before the first run.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Provider {
    FastRefusal,
    SlowFailure,
    QuotaGrid,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EpisodeEnd {
    Opened,
    BudgetRefused,
    Deadline,
    Terminal,
}

#[derive(Debug, PartialEq, Eq)]
struct Episode {
    opens: usize,
    charges: usize,
    ended_ms: u64,
    end: EpisodeEnd,
}

struct Plant {
    provider: Provider,
    recover_ms: Option<u64>,
    extension: Option<(u64, u64)>,
    started: tokio::time::Instant,
    started_std: std::time::Instant,
    cool_ms: u64,
    opens: usize,
    session: Option<GeminiLiveSession>,
}

enum Answer {
    Open,
    Fail(u16, u64),
    Cooling(u64, u64),
}

async fn plant_open(
    plant: std::rc::Rc<std::cell::RefCell<Plant>>,
) -> Result<GeminiLiveSession, Error> {
    let answer = {
        let mut plant = plant.borrow_mut();
        plant.opens += 1;
        let at = u64::try_from(plant.started.elapsed().as_millis()).unwrap();
        let recovered = plant.recover_ms.is_some_and(|recover| at >= recover);
        match plant.provider {
            Provider::FastRefusal if recovered => Answer::Open,
            Provider::FastRefusal => Answer::Fail(503, 100),
            Provider::SlowFailure if recovered => Answer::Open,
            Provider::SlowFailure => Answer::Fail(0, 30_000),
            Provider::QuotaGrid => {
                if let Some((from, until)) = plant.extension
                    && at >= from
                {
                    plant.cool_ms = plant.cool_ms.max(until);
                }
                if at < plant.cool_ms {
                    Answer::Cooling(plant.cool_ms, 0)
                } else if recovered {
                    Answer::Open
                } else {
                    plant.cool_ms = at + 100 + 60_000;
                    Answer::Cooling(plant.cool_ms, 100)
                }
            }
        }
    };
    match answer {
        Answer::Open => {
            tokio::time::sleep(Duration::from_millis(100)).await;
            Ok(plant
                .borrow_mut()
                .session
                .take()
                .expect("one recovery per row"))
        }
        Answer::Fail(status, cost) => {
            tokio::time::sleep(Duration::from_millis(cost)).await;
            Err(crate::gemini::provider_failure_for_test(status))
        }
        Answer::Cooling(until_ms, cost) => {
            tokio::time::sleep(Duration::from_millis(cost)).await;
            let plant = plant.borrow();
            Err(crate::gemini::quota_exhausted_for_test(
                plant.started_std + Duration::from_millis(until_ms),
            ))
        }
    }
}

struct Schedule {
    provider: Provider,
    recover_s: Option<u64>,
    extension: Option<(u64, u64)>,
    limit_s: Option<u64>,
    terminal_s: Option<u64>,
}

fn outage_config() -> AgentConfig {
    load_from_pairs([
        ("LIVEKIT_URL", "wss://example.livekit.cloud"),
        ("LIVEKIT_API_KEY", "devkey"),
        ("LIVEKIT_API_SECRET", "devsecret"),
        ("GOOGLE_API_KEY", "outage-schedule-key"),
    ])
    .unwrap()
}

/// A real socket that has completed setup, standing only for "an open
/// succeeded": it is opened before the clock freezes and is never read, so the
/// rows do not exercise its idle timers.
async fn ready_session() -> (GeminiLiveSession, tokio::task::JoinHandle<()>) {
    let config = outage_config();
    let keys = GeminiKeys::from_config(&config);
    let boot = crate::runtime::bootstrap(&config, "outage-schedule", Some("two-sum"), 45);
    let (fresh, endpoint) = listener().await;
    let server = tokio::spawn(async move {
        scripted_fresh_gemini(fresh, vec![FreshStep::Ready]).await;
    });
    let session = crate::gemini::live_session_with_keys_at(&endpoint, &keys, &boot, None)
        .await
        .unwrap();
    (session, server)
}

/// One episode of the real cold-open loop on a paused clock. The recovering
/// session is opened before the clock is frozen so its socket is real.
async fn outage_episode(schedule: &Schedule) -> Episode {
    let (session, server) = ready_session().await;
    let keys = GeminiKeys::from_config(&outage_config());
    tokio::time::pause();
    let started = tokio::time::Instant::now();
    let plant = std::rc::Rc::new(std::cell::RefCell::new(Plant {
        provider: schedule.provider,
        recover_ms: schedule.recover_s.map(|s| s * 1000),
        extension: schedule
            .extension
            .map(|(from, until)| (from * 1000, until * 1000)),
        started,

        // The paused clock's own coordinates, which drift from the real clock
        // by the logical time earlier rows advanced.
        started_std: started.into_std(),
        cool_ms: 0,
        opens: 0,
        session: Some(session),
    }));
    let mut charges = 1;
    let mut ready = None;
    let work = retry_cold_open_into(
        &keys,
        &mut charges,
        &mut ready,
        || plant_open(plant.clone()),
        |_| async {},
        tokio::time::sleep_until,
    );
    let secs = Duration::from_secs;
    let deadline = schedule.limit_s.map(secs);
    let terminal = schedule.terminal_s.map(secs);
    let bound = match (deadline, terminal) {
        (Some(d), Some(t)) if d <= t => Some((d, EpisodeEnd::Deadline)),
        (_, Some(t)) => Some((t, EpisodeEnd::Terminal)),
        (Some(d), None) => Some((d, EpisodeEnd::Deadline)),
        (None, None) => None,
    };
    let (result, cut) = match bound {
        Some((limit, end)) => (tokio::time::timeout(limit, work).await.ok(), Some(end)),
        None => (Some(work.await), None),
    };

    // The timer wheel rounds each wake up to the next millisecond, so the end
    // is read to the tenth of a second.
    let ended_ms = u64::try_from(started.elapsed().as_millis()).unwrap() / 100 * 100;
    let end = match (&result, cut) {
        (Some(Ok(())), _) => EpisodeEnd::Opened,
        (Some(Err(_)), _) => EpisodeEnd::BudgetRefused,
        (None, Some(end)) => end,
        (None, None) => unreachable!("an unbounded loop returns"),
    };
    let opens = plant.borrow().opens;
    let spare = plant.borrow_mut().session.take();
    tokio::time::resume();
    for session in ready.take().into_iter().chain(spare) {
        session.close().await.unwrap();
    }
    server.await.unwrap();
    Episode {
        opens,
        charges,
        ended_ms,
        end,
    }
}

fn episode(opens: usize, charges: usize, ended_ms: u64, end: EpisodeEnd) -> Episode {
    Episode {
        opens,
        charges,
        ended_ms,
        end,
    }
}

const DEADLINE_S: u64 = 120;

type OutageRow = (&'static str, Schedule, Episode);

fn outage_row(
    name: &'static str,
    provider: Provider,
    recover_s: Option<u64>,
    extension: Option<(u64, u64)>,
    bounds: (Option<u64>, Option<u64>),
    expected: Episode,
) -> OutageRow {
    (
        name,
        Schedule {
            provider,
            recover_s,
            extension,
            limit_s: bounds.0,
            terminal_s: bounds.1,
        },
        expected,
    )
}

fn outage_rows() -> Vec<OutageRow> {
    use EpisodeEnd::*;
    use Provider::*;
    let row = outage_row;
    let none = (None, None);
    let d = (Some(DEADLINE_S), None);
    vec![
        // Control: today's policy, no episode limit.
        row(
            "control_fast_recovers_1s",
            FastRefusal,
            Some(1),
            None,
            none,
            episode(2, 2, 2_200, Opened),
        ),
        row(
            "control_fast_recovers_30s",
            FastRefusal,
            Some(30),
            None,
            none,
            episode(8, 8, 14_800, BudgetRefused),
        ),
        row(
            "control_fast_recovers_60s",
            FastRefusal,
            Some(60),
            None,
            none,
            episode(8, 8, 14_800, BudgetRefused),
        ),
        row(
            "control_fast_recovers_90s",
            FastRefusal,
            Some(90),
            None,
            none,
            episode(8, 8, 14_800, BudgetRefused),
        ),
        row(
            "control_fast_never",
            FastRefusal,
            None,
            None,
            none,
            episode(8, 8, 14_800, BudgetRefused),
        ),
        row(
            "control_slow_recovers_1s",
            SlowFailure,
            Some(1),
            None,
            none,
            episode(2, 2, 32_100, Opened),
        ),
        row(
            "control_slow_recovers_30s",
            SlowFailure,
            Some(30),
            None,
            none,
            episode(2, 2, 32_100, Opened),
        ),
        row(
            "control_slow_recovers_60s",
            SlowFailure,
            Some(60),
            None,
            none,
            episode(3, 3, 64_100, Opened),
        ),
        row(
            "control_slow_recovers_90s",
            SlowFailure,
            Some(90),
            None,
            none,
            episode(4, 4, 96_100, Opened),
        ),
        row(
            "control_slow_never",
            SlowFailure,
            None,
            None,
            none,
            episode(8, 8, 254_000, BudgetRefused),
        ),
        row(
            "control_quota_recovers_1s",
            QuotaGrid,
            Some(1),
            None,
            none,
            episode(2, 2, 60_200, Opened),
        ),
        row(
            "control_quota_recovers_30s",
            QuotaGrid,
            Some(30),
            None,
            none,
            episode(2, 2, 60_200, Opened),
        ),
        row(
            "control_quota_recovers_60s",
            QuotaGrid,
            Some(60),
            None,
            none,
            episode(2, 2, 60_200, Opened),
        ),
        row(
            "control_quota_recovers_90s",
            QuotaGrid,
            Some(90),
            None,
            none,
            episode(3, 3, 120_300, Opened),
        ),
        row(
            "control_quota_never",
            QuotaGrid,
            None,
            None,
            none,
            episode(8, 8, 420_800, BudgetRefused),
        ),
        row(
            "control_quota_recooled_to_150s",
            QuotaGrid,
            Some(1),
            Some((50, 150)),
            none,
            episode(3, 3, 150_100, Opened),
        ),
        // Candidate: the same schedules under a 120-second episode limit.
        row(
            "candidate_fast_recovers_1s",
            FastRefusal,
            Some(1),
            None,
            d,
            episode(2, 2, 2_200, Opened),
        ),
        row(
            "candidate_fast_recovers_30s",
            FastRefusal,
            Some(30),
            None,
            d,
            episode(8, 8, 14_800, BudgetRefused),
        ),
        row(
            "candidate_fast_recovers_60s",
            FastRefusal,
            Some(60),
            None,
            d,
            episode(8, 8, 14_800, BudgetRefused),
        ),
        row(
            "candidate_fast_recovers_90s",
            FastRefusal,
            Some(90),
            None,
            d,
            episode(8, 8, 14_800, BudgetRefused),
        ),
        row(
            "candidate_fast_never",
            FastRefusal,
            None,
            None,
            d,
            episode(8, 8, 14_800, BudgetRefused),
        ),
        row(
            "candidate_slow_recovers_1s",
            SlowFailure,
            Some(1),
            None,
            d,
            episode(2, 2, 32_100, Opened),
        ),
        row(
            "candidate_slow_recovers_30s",
            SlowFailure,
            Some(30),
            None,
            d,
            episode(2, 2, 32_100, Opened),
        ),
        row(
            "candidate_slow_recovers_60s",
            SlowFailure,
            Some(60),
            None,
            d,
            episode(3, 3, 64_100, Opened),
        ),
        row(
            "candidate_slow_recovers_90s",
            SlowFailure,
            Some(90),
            None,
            d,
            episode(4, 4, 96_100, Opened),
        ),
        row(
            "candidate_slow_never",
            SlowFailure,
            None,
            None,
            d,
            episode(4, 4, 120_000, Deadline),
        ),
        row(
            "candidate_quota_recovers_1s",
            QuotaGrid,
            Some(1),
            None,
            d,
            episode(2, 2, 60_200, Opened),
        ),
        row(
            "candidate_quota_recovers_30s",
            QuotaGrid,
            Some(30),
            None,
            d,
            episode(2, 2, 60_200, Opened),
        ),
        row(
            "candidate_quota_recovers_60s",
            QuotaGrid,
            Some(60),
            None,
            d,
            episode(2, 2, 60_200, Opened),
        ),
        row(
            "candidate_quota_recovers_90s",
            QuotaGrid,
            Some(90),
            None,
            d,
            episode(2, 3, 120_000, Deadline),
        ),
        row(
            "candidate_quota_never",
            QuotaGrid,
            None,
            None,
            d,
            episode(2, 3, 120_000, Deadline),
        ),
        row(
            "candidate_quota_recooled_to_150s",
            QuotaGrid,
            Some(1),
            Some((50, 150)),
            d,
            episode(2, 3, 120_000, Deadline),
        ),
        // The quota grid lands the 90-second recovery at 120.3 seconds, so only
        // a limit past that preserves it.
        row(
            "candidate_quota_recovers_90s_limit_121s",
            QuotaGrid,
            Some(90),
            None,
            (Some(121), None),
            episode(3, 3, 120_300, Opened),
        ),
        // Terminal controls are earlier limits and win when they come first.
        row(
            "control_slow_never_terminal_50s",
            SlowFailure,
            None,
            None,
            (None, Some(50)),
            episode(2, 2, 50_000, Terminal),
        ),
        row(
            "candidate_slow_never_terminal_50s",
            SlowFailure,
            None,
            None,
            (Some(DEADLINE_S), Some(50)),
            episode(2, 2, 50_000, Terminal),
        ),
        row(
            "control_slow_never_terminal_130s",
            SlowFailure,
            None,
            None,
            (None, Some(130)),
            episode(5, 5, 130_000, Terminal),
        ),
        row(
            "candidate_slow_never_terminal_130s",
            SlowFailure,
            None,
            None,
            (Some(DEADLINE_S), Some(130)),
            episode(4, 4, 120_000, Deadline),
        ),
        row(
            "control_quota_never_terminal_100s",
            QuotaGrid,
            None,
            None,
            (None, Some(100)),
            episode(2, 3, 100_000, Terminal),
        ),
        row(
            "candidate_quota_never_terminal_100s",
            QuotaGrid,
            None,
            None,
            (Some(DEADLINE_S), Some(100)),
            episode(2, 3, 100_000, Terminal),
        ),
        row(
            "control_quota_never_terminal_130s",
            QuotaGrid,
            None,
            None,
            (None, Some(130)),
            episode(3, 4, 130_000, Terminal),
        ),
        row(
            "candidate_quota_never_terminal_130s",
            QuotaGrid,
            None,
            None,
            (Some(DEADLINE_S), Some(130)),
            episode(2, 3, 120_000, Deadline),
        ),
    ]
}

#[tokio::test(flavor = "current_thread")]
async fn provider_outage_schedules_follow_the_declared_table() {
    if isolated_checked(
        "provider_outage_schedules_follow_the_declared_table",
        |output| {
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert_eq!(
                stderr
                    .lines()
                    .filter(|line| line.starts_with("outage row: "))
                    .count(),
                outage_rows().len()
            );
        },
    ) {
        return;
    }
    let rows = outage_rows();
    assert_eq!(rows.len(), 41);
    for (name, schedule, expected) in rows {
        let observed = outage_episode(&schedule).await;
        eprintln!(
            "outage row: {name} opens={} charges={} ended_ms={} end={:?}",
            observed.opens, observed.charges, observed.ended_ms, observed.end
        );
        assert_eq!(observed, expected, "{name}");
    }
}

// A repeated GoAway is a series of separate episodes, each healthy and short.
// An episode limit never fires on any of them, so the series is bounded only by
// the restart budget and its age reset (see the age table above): sockets that
// live 61 seconds and are replaced idle are never refused over an hour, and
// replacements that owe a reply are refused on the ninth close.
#[tokio::test(flavor = "current_thread")]
async fn repeated_goaway_episodes_are_not_bounded_by_an_episode_limit() {
    if isolated("repeated_goaway_episodes_are_not_bounded_by_an_episode_limit") {
        return;
    }
    let one = outage_episode(&Schedule {
        provider: Provider::FastRefusal,
        recover_s: Some(0),
        extension: None,
        limit_s: Some(DEADLINE_S),
        terminal_s: None,
    })
    .await;
    assert_eq!(one, episode(1, 1, 100, EpisodeEnd::Opened));
    let healthy = run_restart_budget(59, 61, SocketEnd::Idle);
    assert_eq!((healthy.opens, healthy.refused_at_logical_s), (59, None));
    let owed = run_restart_budget(59, 61, SocketEnd::OwedReply);
    assert_eq!((owed.opens, owed.refused_at_logical_s), (8, Some(549)));
}

// Trailing output after a finished reply, and a tool call consumed while a
// prompt is owed. Declared before running. Each row delivers events to the live
// socket through the room loop's handler, rotates, and counts the briefing
// frames on the replacement that ask for a reply (turnComplete true).
//
//   row                                  events                      gen  asks
//   finished reply                       words, done                 no   0
//   trailing transcript after the end    words, done, words          no   0
//   same, with a continuation fallback   words, done, fallback,
//                                        words                       no   0
//   reply cut off before its end         words                       yes  1
//   next generation's text               words, done, text           yes  1
//   trailing transcript, then a tool     words, done, words, tool    yes  1
//   prompt owed after the end            words, done, prompt, words  yes  1
//   candidate spoke after the end        words, done, speech, words  yes  1
//   window over before the transcript    words, done, expire, words  yes  1
//   interrupted after the end            words, done, interrupted,
//                                        words                       yes  1
//
// The provider can send a generation's output transcription after its
// completion, and nothing in it says whether it is that or the next
// generation's first words. Before the fix the trailing-transcript row asked
// for a reply that had already been given, because the transcript set
// `generating` and no completion followed to clear it. Every row where a reply
// can really be owed keeps `generating`: the limit is that a transcript with
// something owed, or heard after the candidate spoke, is taken as the answer it
// may be rather than dropped on a guess. In every row the words answer a prompt
// and clear the continuation fallback.
#[derive(Clone, Copy, Debug)]
enum TrailStep {
    Words,
    Done,
    Text,
    Tool,
    Prompt,
    Speech,
    Expire,
    Fallback,
    Interrupt,
}

impl Rig {
    async fn receive_trail_step(&mut self, step: TrailStep) {
        let event = match step {
            TrailStep::Words => crate::gemini::GeminiEvent::OutputTranscript("Okay.".into()),
            TrailStep::Done => crate::gemini::GeminiEvent::TurnComplete,
            TrailStep::Text => crate::gemini::GeminiEvent::Text("Next.".into()),
            TrailStep::Tool => crate::gemini::GeminiEvent::ToolCall(vec![tool_call("call-1")]),
            TrailStep::Interrupt => crate::gemini::GeminiEvent::Interrupted,
            TrailStep::Prompt => {
                self.turn.activity.latency.prompt(Instant::now());
                self.turn.activity.owe_prompt(Instant::now(), None);
                return;
            }
            TrailStep::Speech => {
                self.turn
                    .activity
                    .note_candidate_finished(Instant::now(), false);
                return;
            }
            TrailStep::Expire => {
                // The whole exchange moves into the past, so only the window,
                // not the candidate's silence, separates it from now.
                let age = crate::livekit::turn::LATE_TRANSCRIPT_GRACE + Duration::from_secs(1);
                let activity = &mut self.turn.activity;
                *activity.turn_completed_at.as_mut().unwrap() -= age;
                activity.last_user_speech -= age;
                return;
            }
            TrailStep::Fallback => {
                self.turn
                    .activity
                    .arm_thinking_reply_fallback(Instant::now(), "owed");
                return;
            }
        };
        let mut context = self.turn.context(
            &mut self.output,
            &mut self.gemini,
            &mut self.board,
            &mut self.media,
        );
        session::handle_gemini_event(&self.room, &mut context, event, Interruptible::Yes)
            .await
            .unwrap();
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_trailing_transcript_does_not_re_ask_a_finished_reply_on_rotation() {
    if isolated("a_trailing_transcript_does_not_re_ask_a_finished_reply_on_rotation") {
        return;
    }
    use TrailStep::*;
    // (label, events, `generating` before the rotation, asks after it)
    let rows: [(&str, &[TrailStep], bool, usize); 10] = [
        ("finished reply", &[Words, Done], false, 0),
        ("trailing transcript", &[Words, Done, Words], false, 0),
        (
            "trailing with a fallback",
            &[Words, Done, Fallback, Words],
            false,
            0,
        ),
        ("cut off before its end", &[Words], true, 1),
        ("next generation text", &[Words, Done, Text], true, 1),
        ("trailing then tool", &[Words, Done, Words, Tool], true, 1),
        ("prompt owed", &[Words, Done, Prompt, Words], true, 1),
        ("candidate spoke", &[Words, Done, Speech, Words], true, 1),
        ("window over", &[Words, Done, Expire, Words], true, 1),
        ("interrupted", &[Words, Done, Interrupt, Words], true, 1),
    ];
    for (label, steps, generating, asks) in rows {
        let checkpoint = serde_json::json!({"resumable": true, "newHandle": "trail-handle"});
        let (fresh, endpoint) = listener().await;
        let mut rig =
            Rig::with_old_resumption("ws://127.0.0.1:9/".into(), Some(11), Some(checkpoint)).await;
        rig.endpoint = endpoint;
        let old_reader = rig.gemini.inspect_reader_for_test().unwrap();
        wait_for_received_usage(&old_reader).await;
        for step in steps {
            rig.receive_trail_step(*step).await;
        }
        assert_eq!(rig.turn.activity.generating, generating, "{label}");
        assert!(!rig.turn.activity.owes_prompt(), "{label}: words answer it");
        assert!(
            rig.turn.activity.socket.thinking_reply_fallback.is_none(),
            "{label}: the words settle the fallback"
        );
        let server = tokio::spawn(recording_fresh_gemini(fresh, vec![FreshStep::Ready]));
        assert!(rig.run().await.is_continue(), "{label}");
        let _ = rig.old_frames().await;
        let seen = server.await.unwrap();
        assert_eq!(seen.len(), 1, "{label}");
        let turns = briefing_turns(&seen[0].1);
        assert_eq!(
            turns.iter().filter(|asked| **asked).count(),
            asks,
            "{label}: {turns:?}"
        );
    }
}

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
