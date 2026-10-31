// Part of recovery.rs: included in place, so every item here shares that file's
// module.

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
    let seen_accepts = std::sync::Arc::clone(&accepts);
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
                            crate::livekit::tests::fixture_session_at(endpoint, keys, boot, None)
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
                    let notice = std::sync::Arc::clone(&notice);
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
        let server_accepts = std::sync::Arc::clone(&accepts);
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
            rig.loops.presence.test_now = Some(std::sync::Arc::clone(&presence_now));
        }
        let (probe, mut observed) = mpsc::unbounded_channel();
        rig.loops.recovery_terminal = Some(probe);
        let (reader, mut readers) = mpsc::unbounded_channel();
        rig.loops.recovery_reader = Some(reader);
        let (unused, _) = mpsc::unbounded_channel();
        let mut room = Some(std::mem::replace(&mut rig._room_sender, unused));
        let tx = rig.tx.clone();
        let recorder = std::sync::Arc::clone(&rig.recorder);
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
        retry_usage(out, false);
    }) {
        return;
    }
    retry_wait_case_matrix(RetryWait::WaitNotice).await;
}
#[tokio::test(flavor = "current_thread")]
async fn cold_retry_backoff_remains_cancellable() {
    if isolated_checked("cold_retry_backoff_remains_cancellable", |out| {
        retry_usage(out, false);
    }) {
        return;
    }
    retry_wait_case_matrix(RetryWait::Backoff).await;
}
#[tokio::test(flavor = "current_thread")]
async fn cold_retry_attempt_notice_cancels_pending_open() {
    if isolated_checked("cold_retry_attempt_notice_cancels_pending_open", |out| {
        retry_usage(out, false);
    }) {
        return;
    }
    retry_wait_case_matrix(RetryWait::AttemptPending).await;
}
#[tokio::test(flavor = "current_thread")]
async fn cold_retry_attempt_notice_owns_completed_setup() {
    if isolated_checked("cold_retry_attempt_notice_owns_completed_setup", |out| {
        retry_usage(out, true);
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
                rig.loops.presence.test_now = Some(std::sync::Arc::clone(&presence_now));
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
        let bearer = "authorization: bearer ";
        let token = header
            .lines()
            .find_map(|line| {
                line.get(..bearer.len())
                    .filter(|head| head.eq_ignore_ascii_case(bearer))
                    .map(|_| line[bearer.len()..].trim())
            })
            .expect("the removal is authorized");

        // Signed now, for a short while: a clock read in the wrong unit would
        // sign a token that has already expired or one that never does.
        let claims: serde_json::Value = serde_json::from_slice(
            &base64::Engine::decode(
                &base64::engine::general_purpose::URL_SAFE_NO_PAD,
                token.split('.').nth(1).unwrap(),
            )
            .unwrap(),
        )
        .unwrap();
        let (exp, now) = (
            claims["exp"].as_u64().unwrap(),
            crate::current_epoch_seconds(),
        );
        let ttl = crate::token::TOKEN_TTL_SECONDS;
        assert!(
            exp >= now && exp <= now + ttl && exp + 60 > now + ttl,
            "exp {exp} against now {now}"
        );
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
            rig.loops.presence.test_now = Some(std::sync::Arc::clone(&presence_now));
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
                    control(&tx, serde_json::json!({"type":"thinking", "thinking":true}));
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
