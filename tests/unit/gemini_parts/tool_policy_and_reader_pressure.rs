// Part of gemini.rs: included in place, so every item here shares that file's
// module.

#[test]
fn a_self_review_check_with_multiple_policy_violations_is_dropped_once() {
    let mut report = valid_report();
    report["improvementPlan"][0]["selfReview"] = json!([
        "Speak clearly in English without nervous filler words.",
        "Verify the loop invariant"
    ]);
    let salvage = salvage_report(
        report,
        0,
        report_problem(),
        rounds(true),
        InterviewMode::Coding,
        true,
    )
    .unwrap();
    assert_eq!(salvage.removed.checks, 1);
    assert_eq!(
        salvage.report["improvementPlan"][0]["selfReview"],
        json!(["Verify the loop invariant"])
    );
}

/// The phase judge asks for JSON under its own instruction and hands back the
/// text the server then checks. One key cannot tell the pools apart; which
/// pool it spends, and what a failure does to it, is
/// `a_rate_limited_phase_judge_does_not_cool_the_report_key`'s to show.
#[tokio::test]
async fn the_phase_judge_requests_json_under_its_own_instruction() {
    let keys = GeminiKeys::single("phase-judge-key");
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&bodies);
    let app = axum::Router::new().route(
        "/",
        axum::routing::post(move |axum::Json(body): axum::Json<Value>| {
            let seen = Arc::clone(&seen);
            async move {
                seen.lock().unwrap().push(body);
                axum::Json(
                    json!({"candidates":[{"content":{"parts":[{"text":"{\"steps\": []}"}]}}]}),
                )
            }
        }),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    assert_eq!(
        generate_phase_judgment_at(&keys, &url, "prompt", "test-room")
            .await
            .unwrap(),
        "{\"steps\": []}"
    );
    server.abort();
    let body = bodies.lock().unwrap()[0].clone();
    assert_eq!(
        body["generationConfig"]["responseMimeType"],
        "application/json"
    );
    assert_eq!(
        body["systemInstruction"]["parts"][0]["text"],
        crate::agent::phase_judge_system_instruction()
    );
    assert_eq!(body["contents"][0]["parts"][0]["text"], "prompt");
}

/// A rate limit on the phase judge leaves the key on the report surface, where
/// the same limit on an interim review cools it: the judge runs dozens of
/// times an interview and must not be what leaves the final report keyless.
#[tokio::test]
async fn a_rate_limited_phase_judge_does_not_cool_the_report_key() {
    for (judge, cools) in [(true, false), (false, true)] {
        let first = format!("side-quota-{judge}-first");
        let second = format!("side-quota-{judge}-second");
        let config = live_config(&[("GOOGLE_API_KEYS", &format!("{first},{second}"))]);
        let keys = GeminiKeys::from_config(&config);
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let app = axum::Router::new().route(
            "/",
            axum::routing::post(|| async { axum::http::StatusCode::TOO_MANY_REQUESTS }),
        );
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let result = if judge {
            generate_phase_judgment_at(&keys, &url, "prompt", "test-room").await
        } else {
            generate_interim_review_at(&keys, &url, "prompt", "test-room").await
        };
        assert!(result.is_err());
        server.abort();
        let expected = if cools { &second } else { &first };
        assert_eq!(&keys.select_report().unwrap(), expected, "judge={judge}");
    }
}

#[test]
fn provider_voice_activity_parses_only_known_activity_types() {
    for (kind, expected) in [
        ("ACTIVITY_START", Some(true)),
        ("ACTIVITY_END", Some(false)),
        ("UNKNOWN", None),
    ] {
        let parsed = parse_server_message(
            &serde_json::json!({"voiceActivity": {"type": kind, "audioOffset": "0.320s"}})
                .to_string(),
        );
        assert_eq!(
            parsed.events,
            expected
                .into_iter()
                .map(|active| GeminiEvent::CandidateVoiceActivity {
                    active,
                    audio_offset: Some(Duration::from_millis(320))
                })
                .collect::<Vec<_>>()
        );
    }
}

#[test]
fn coding_only_tool_schema_excludes_behavioral_phases_on_both_surfaces() {
    // Only a whiteboard is offered the tool at all.
    for mode in [InterviewMode::Coding, InterviewMode::Whiteboard] {
        if !crate::agent::evidence_tool_declared(mode) {
            let tools =
                live_tool_declarations(crate::agent::InterviewLoop::CodingBehavioral, mode, false);
            assert!(
                tools
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|tool| tool["name"] != TOOL_RECORD_FRAMEWORK_EVIDENCE)
            );
            continue;
        }
        let coding = live_tool_declarations(crate::agent::InterviewLoop::CodingOnly, mode, false);
        let full =
            live_tool_declarations(crate::agent::InterviewLoop::CodingBehavioral, mode, false);
        let find = |tools: Value| {
            tools
                .as_array()
                .unwrap()
                .iter()
                .find(|tool| tool["name"] == TOOL_RECORD_FRAMEWORK_EVIDENCE)
                .unwrap()
                .clone()
        };
        let coding = find(coding);
        let full = find(full);
        assert_eq!(
            coding["parameters"]["properties"]["phase"]["enum"],
            json!([
                "repeat",
                "example",
                "algorithm",
                "coding",
                "test",
                "optimizations"
            ])
        );
        assert_eq!(
            full["parameters"]["properties"]["phase"]["enum"]
                .as_array()
                .unwrap()
                .len(),
            10
        );
        assert!(!coding["description"].as_str().unwrap().contains("STAR"));
        assert_eq!(
            coding["parameters"]["properties"]["source"],
            full["parameters"]["properties"]["source"]
        );
        assert_eq!(
            coding["parameters"]["required"],
            full["parameters"]["required"]
        );
    }
}

#[tokio::test]
async fn bounded_http_diagnostic_preserves_real_connect_error_and_hides_url() {
    // A connected socket reserves its source port without accepting new
    // connections. A merely bound socket times out on macOS; dropping a
    // listener instead lets another test take its port before the request.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let reserved = tokio::net::TcpStream::connect(listener.local_addr().unwrap())
        .await
        .unwrap();
    let (_peer, _) = listener.accept().await.unwrap();
    let address = reserved.local_addr().unwrap();
    let error = generate_content_once(
        "SECRET_KEY_SENTINEL",
        &format!("http://{address}/SECRET_URL_SENTINEL"),
        &json!({}),
        Duration::from_secs(1),
        "diagnostic",
    )
    .await
    .unwrap_err();
    assert!(error.downcast_ref::<reqwest::Error>().unwrap().is_connect());
    assert!(is_retryable(error.as_ref()));
    let detail = http_failure_diagnostic(
        error.as_ref(),
        "send",
        Duration::from_millis(7),
        Duration::from_secs(1),
    );
    assert_eq!(detail["connect"], true);
    assert_eq!(detail["elapsed_ms"], 7);
    assert_eq!(detail["timeout_ms"], 1000);
    assert!(detail["source_depth"].as_u64().unwrap() <= 8);
    for secret in ["SECRET_KEY_SENTINEL", "SECRET_URL_SENTINEL", "127.0.0.1"] {
        assert!(!detail.to_string().contains(secret));
    }
}

#[tokio::test]
async fn bounded_http_diagnostic_distinguishes_actual_timeout_decode_and_provider() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    for scenario in ["timeout", "decode", "provider"] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0; 4096];
            let _ = socket.read(&mut request).await.unwrap();
            if scenario == "timeout" {
                tokio::time::sleep(Duration::from_millis(200)).await;
                return;
            }
            let (status, body) = if scenario == "decode" {
                ("200 OK", "not JSON")
            } else {
                (
                    "503 Service Unavailable",
                    "{\"error\":{\"message\":\"SECRET_BODY_SENTINEL\"}}",
                )
            };
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            socket.write_all(response.as_bytes()).await.unwrap();
        });
        let limit = if scenario == "timeout" {
            Duration::from_millis(50)
        } else {
            Duration::from_secs(1)
        };
        let error = generate_content_once(
            "SECRET_KEY_SENTINEL",
            &format!("http://{address}/SECRET_URL_SENTINEL"),
            &json!({}),
            limit,
            "diagnostic",
        )
        .await
        .unwrap_err();
        let stage = if scenario == "decode" {
            "decode"
        } else if scenario == "provider" {
            "http_status"
        } else {
            "send"
        };
        let detail = http_failure_diagnostic(error.as_ref(), stage, limit, limit);
        match scenario {
            "timeout" => {
                assert_eq!(detail["timeout"], true);
                assert!(is_retryable(error.as_ref()));
            }
            "decode" => {
                assert_eq!(detail["decode"], true);
                assert!(!is_retryable(error.as_ref()));
            }
            _ => {
                assert_eq!(detail["status"], 503);
                assert!(error.downcast_ref::<ApiFailure>().is_some());
                assert!(is_retryable(error.as_ref()));
            }
        }
        for secret in [
            "SECRET_KEY_SENTINEL",
            "SECRET_URL_SENTINEL",
            "SECRET_BODY_SENTINEL",
            "127.0.0.1",
        ] {
            assert!(!detail.to_string().contains(secret));
        }
        server.await.unwrap();
    }
}

#[test]
fn bounded_http_diagnostic_emission_child() {
    if std::env::var_os("CODETRIAL_HTTP_DIAGNOSTIC_TEST_CHILD").is_none() {
        return;
    }
    bounded_http_diagnostic_preserves_real_connect_error_and_hides_url();
    bounded_http_diagnostic_distinguishes_actual_timeout_decode_and_provider();
}

#[test]
fn bounded_http_diagnostic_actual_stderr_preserves_stage_and_privacy() {
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "gemini::tests::bounded_http_diagnostic_emission_child",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("CODETRIAL_HTTP_DIAGNOSTIC_TEST_CHILD", "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "child failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
    let stderr = String::from_utf8(output.stderr).unwrap();
    let events = stderr
        .lines()
        .filter_map(|line| line.strip_prefix("codetrial http_failure "))
        .map(|json| serde_json::from_str::<Value>(json).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(events.len(), 4, "{stderr}");
    assert_eq!(
        events
            .iter()
            .filter(|event| event["stage"] == "send")
            .count(),
        2
    );
    let connect = events
        .iter()
        .find(|event| event["connect"] == true)
        .unwrap();
    assert_eq!(connect["stage"], "send");
    let timeout = events
        .iter()
        .find(|event| event["timeout"] == true)
        .unwrap();
    assert_eq!(timeout["stage"], "send");
    assert_eq!(timeout["timeout_ms"], 50);
    assert!(timeout["elapsed_ms"].as_u64().unwrap() >= 40);
    let decode = events
        .iter()
        .find(|event| event["stage"] == "decode")
        .unwrap();
    assert_eq!(decode["decode"], true);
    let provider = events
        .iter()
        .find(|event| event["stage"] == "http_status")
        .unwrap();
    assert_eq!(provider["status"], 503);
    for event in &events {
        assert!(event["elapsed_ms"].as_u64().unwrap() < 5000);
        assert!(event["source_depth"].as_u64().unwrap() <= 8);
        assert!(event["timeout_ms"] == 50 || event["timeout_ms"] == 1000);
    }
    for sentinel in [
        "SECRET_KEY_SENTINEL",
        "SECRET_URL_SENTINEL",
        "SECRET_BODY_SENTINEL",
        "127.0.0.1",
    ] {
        assert!(!stderr.contains(sentinel));
    }
}

/// A reply is owed from the first sign of candidate speech, before any output:
/// a completed client turn sent in that gap would interrupt it. Only a turn's
/// completion clears it, unless an interruption beside it says the candidate
/// is speaking again; trailing output transcription says nothing either way.
#[test]
fn busy_spans_candidate_speech_to_turn_completion() {
    let busy = |frame: &str| parse_server_message(frame).busy;
    assert_eq!(
        busy(r#"{"voiceActivity":{"type":"ACTIVITY_END"}}"#),
        Some(true)
    );
    assert_eq!(
        busy(r#"{"serverContent":{"inputTranscription":{"text":"done"}}}"#),
        Some(true)
    );
    assert_eq!(
        busy(r#"{"serverContent":{"modelTurn":{"parts":[{"text":"ok"}]}}}"#),
        Some(true)
    );
    assert_eq!(
        busy(r#"{"serverContent":{"turnComplete":true}}"#),
        Some(false)
    );
    assert_eq!(
        busy(r#"{"serverContent":{"interrupted":true,"turnComplete":true}}"#),
        Some(true)
    );
    assert_eq!(
        busy(r#"{"serverContent":{"outputTranscription":{"text":"late"}}}"#),
        None
    );
}

/// A reply this server asked for stays owed through the longest reply
/// deadline: a later frame of evidence extends the window but never shortens
/// it to its own ten seconds.
#[test]
fn evidence_never_shortens_a_requested_reply_window() {
    let busy = Mutex::new(None);
    extend_busy(&busy, BUSY_REQUEST_LIMIT);
    let requested = busy.lock().unwrap().unwrap();
    extend_busy(&busy, BUSY_EVIDENCE_LIMIT);
    assert_eq!(busy.lock().unwrap().unwrap(), requested);
    assert!(requested >= std::time::Instant::now() + Duration::from_secs(100));

    let observed = Mutex::new(None);
    extend_busy(&observed, BUSY_EVIDENCE_LIMIT);
    assert!(
        observed.lock().unwrap().unwrap() < std::time::Instant::now() + Duration::from_secs(11)
    );
}

/// A busy deadline holds until it arrives, and not at the instant itself.
#[test]
fn a_busy_deadline_lapses_at_its_own_instant() {
    let now = std::time::Instant::now();
    assert!(busy_at(Some(now + Duration::from_millis(1)), now));
    assert!(!busy_at(Some(now), now));
    assert!(!busy_at(None, now));
}

/// The diagnostic walks at most eight sources: an I/O cause at the eighth is
/// named, one at the ninth is not, so a cyclic or endless chain stays bounded.
#[test]
fn http_diagnostic_walks_eight_sources_and_no_further() {
    #[derive(Debug)]
    struct Wrap(Box<dyn std::error::Error + Send + Sync>);
    impl std::fmt::Display for Wrap {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("wrapped")
        }
    }
    impl std::error::Error for Wrap {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            Some(self.0.as_ref())
        }
    }
    let chain = |depth: usize| {
        let mut error: Box<dyn std::error::Error + Send + Sync> =
            Box::new(io::Error::from(io::ErrorKind::ConnectionReset));
        for _ in 0..depth {
            error = Box::new(Wrap(error));
        }
        error
    };
    let diagnose = |depth: usize| {
        http_failure_diagnostic(
            chain(depth).as_ref(),
            "send",
            Duration::from_millis(1),
            Duration::from_secs(1),
        )
    };
    let reached = diagnose(8);
    assert_eq!(reached["io_kind"], "ConnectionReset");
    assert_eq!(reached["source_depth"], 8);
    let beyond = diagnose(9);
    assert_eq!(beyond["io_kind"], serde_json::Value::Null);
    assert_eq!(beyond["source_depth"], 8);
}

#[test]
fn reconnect_setup_preserves_resolved_voice_and_accent() {
    let config = live_config(&[("GEMINI_VOICE", "Aoede")]);
    for voice in crate::agent::INTERVIEWER_VOICES {
        for accent in crate::agent::INTERVIEWER_ACCENTS {
            let boot = crate::runtime::bootstrap_with_rounds(
                &config,
                "room",
                Some("two-sum"),
                45,
                crate::runtime::RuntimeOptions {
                    voice: Some(voice),
                    accent: Some(accent),
                    ..Default::default()
                },
            );
            assert_eq!(boot.accent, Some(*accent));
            for resume in [None, Some("resume-handle"), None] {
                let setup = live_setup_message(&boot, resume);
                assert_eq!(
                    setup["setup"]["generationConfig"]["speechConfig"]["voiceConfig"]["prebuiltVoiceConfig"]
                        ["voiceName"],
                    *voice
                );
                assert_eq!(
                    setup["setup"]["systemInstruction"]["parts"][0]["text"],
                    boot.instructions
                );
                assert!(
                    boot.instructions
                        .contains(&format!("natural {accent} accent"))
                );
            }
        }
    }
}

#[tokio::test]
async fn queued_transcripts_keep_the_provider_receipt_time() {
    let config = live_config(&[]);
    let boot = bootstrap(&config, "interview-fixed", Some("two-sum"), 45);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut socket = accept_async(socket).await.unwrap();
        socket.next().await.unwrap().unwrap();
        for frame in [
            r#"{"setupComplete":{}}"#,
            r#"{"serverContent":{"inputTranscription":{"text":"finished"},"turnComplete":true}}"#,
        ] {
            socket.send(Message::Text(frame.into())).await.unwrap();
        }
        std::future::pending::<()>().await;
    });
    let mut session = open_fixture_at(&url, &boot, None).await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while session.events.len() < 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let queued_at = std::time::Instant::now();
    assert!(matches!(
        session.next_event().await,
        Some(GeminiEvent::InputTranscript(_))
    ));
    let transcript_at = session.event_received_at().unwrap();
    assert!(transcript_at <= queued_at);
    assert!(matches!(
        session.next_event().await,
        Some(GeminiEvent::TurnComplete)
    ));
    assert_eq!(session.event_received_at(), Some(transcript_at));
    session.shutdown().await.unwrap();
    server.abort();
}

/// The acknowledgement is played straight into the output track, so audio at
/// any other rate is refused rather than played at the wrong speed.
#[test]
fn a_tool_acknowledgement_is_taken_only_at_the_output_rate() {
    let response = |mime: &str| {
        json!({"candidates": [{"content": {"parts": [{"inlineData": {
            "mimeType": mime, "data": STANDARD.encode([1u8, 0, 2, 0])
        }}]}}]})
    };
    assert_eq!(
        tool_ack_pcm(&response("audio/L16;codec=pcm;rate=24000"), 24_000).unwrap(),
        [1, 0, 2, 0]
    );
    assert!(tool_ack_pcm(&response("audio/pcm;rate=24000"), 24_000).is_ok());
    // What the speech model actually sends: lower case, spaced, with channels.
    assert!(tool_ack_pcm(&response("audio/l16; rate=24000; channels=1"), 24_000).is_ok());
    assert!(tool_ack_pcm(&response("audio/L16;codec=pcm;rate=16000"), 24_000).is_err());
    assert!(tool_ack_pcm(&response("audio/L16;codec=pcm;rate=240000"), 24_000).is_err());
    assert!(tool_ack_pcm(&response("audio/wav;rate=24000"), 24_000).is_err());
    assert!(tool_ack_pcm(&json!({}), 24_000).is_err());
}

/// The acknowledgement is asked for in the interview's accent when it has one,
/// so it does not sound like another speaker in front of the reply.
#[test]
fn a_tool_acknowledgement_is_spoken_in_the_interviews_accent() {
    assert_eq!(
        tool_ack_text(None),
        "Say briefly, in a natural, thinking tone: Mm, okay."
    );
    assert_eq!(
        tool_ack_text(Some("Irish")),
        "Say briefly, in a natural, thinking tone, with a natural Irish accent: Mm, okay."
    );
}

/// The acknowledgement is one speech request in the session's voice, with the
/// key in a header, and comes back as the decoded samples.
#[tokio::test]
async fn a_tool_acknowledgement_is_one_speech_request_in_the_sessions_voice() {
    let keys = GeminiKeys::single("tool-ack-key");
    let requests = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&requests);
    let app = axum::Router::new().route(
        "/",
        axum::routing::post(
            move |headers: axum::http::HeaderMap, axum::Json(body): axum::Json<Value>| {
                let seen = Arc::clone(&seen);
                async move {
                    let key = headers["x-goog-api-key"].to_str().unwrap().to_string();
                    seen.lock().unwrap().push((key, body));
                    axum::Json(
                        json!({"candidates": [{"content": {"parts": [{"inlineData": {
                            "mimeType": "audio/L16;codec=pcm;rate=24000",
                            "data": STANDARD.encode([7u8, 0, 9, 0])
                        }}]}}]}),
                    )
                }
            },
        ),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let clip = tool_ack_clip_at(&keys, &url, "Charon", None, 24_000)
        .await
        .unwrap();
    server.abort();
    assert_eq!(clip, [7, 0, 9, 0]);
    assert_eq!(requests.lock().unwrap().len(), 1);
    let (key, body) = requests.lock().unwrap()[0].clone();
    assert_eq!(key, "tool-ack-key");
    assert_eq!(body["contents"][0]["parts"][0]["text"], tool_ack_text(None));
    assert_eq!(
        body["generationConfig"]["responseModalities"],
        json!(["AUDIO"])
    );
    assert_eq!(
        body["generationConfig"]["speechConfig"]["voiceConfig"]["prebuiltVoiceConfig"]["voiceName"],
        "Charon"
    );
}

// Channel pressure evidence. Declared before running:
// * a consumer that is held while the peer sends one event more than the
//   channel holds leaves capacity 256, high water 256, one blocked send,
//   257 received and 0 consumed, and the reader alive past the 45 second read
//   limit, because a reader parked on the channel is not idle;
// * the wait counts exactly the logical time that passed, and keeps counting
//   while the send is pending;
// * releasing the consumer ends the wait, and draining everything leaves the
//   high water at 256, not at the 0 the channel holds then;
// * only a reader that then goes unread for the limit ends as `read_idle`,
//   with the blocked time unchanged;
// * cancelling the reader mid-wait, by stopping it or by dropping the session,
//   closes the wait at that moment instead of leaving it growing;
// * a silent peer that never blocked ends as `read_idle` with no blocked time;
// * the summary is handed out once.

/// A peer that completes setup, sends `frames` audio events and goes quiet.
async fn flooding_server(frames: usize) -> std::net::SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut socket = accept_async(socket).await.unwrap();
        let _ = socket.next().await;
        socket
            .send(Message::Text(r#"{"setupComplete":{}}"#.into()))
            .await
            .unwrap();
        let audio = r#"{"serverContent":{"modelTurn":{"parts":[{"inlineData":{"mimeType":"audio/pcm","data":"AAAA"}}]}}}"#;
        for _ in 0..frames {
            socket.send(Message::Text(audio.into())).await.unwrap();
        }
        std::future::pending::<()>().await;
    });
    address
}

/// Opens a session against [`flooding_server`] and waits, in real time, until
/// the reader has decoded `frames` events. The clock is still running here, so
/// the caller pauses it only once the reader is parked.
async fn flooded_session(frames: usize) -> GeminiLiveSession {
    let config = live_config(&[]);
    let boot = bootstrap(&config, "interview-fixed", Some("two-sum"), 45);
    let address = flooding_server(frames).await;
    let session = open_fixture_at(&format!("ws://{address}"), &boot, None)
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while session.pressure_for_test().received < frames as u64 {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .expect("the reader decodes every frame the peer sent");
    session
}

async fn let_the_reader_run() {
    for _ in 0..5 {
        tokio::task::yield_now().await;
    }
}

#[tokio::test]
async fn a_held_consumer_fills_every_slot_and_the_wait_is_counted_exactly() {
    let mut session = flooded_session(GEMINI_EVENT_QUEUE + 1).await;
    let full = session.pressure_for_test();
    assert_eq!(full.capacity, 256);
    assert_eq!(full.high_water, 256);
    assert_eq!(full.blocked_sends, 1);
    assert_eq!((full.received, full.consumed), (257, 0));

    // Frozen at the moment the clock stops, so everything after is exact.
    tokio::time::pause();
    let frozen = session.pressure_for_test();
    assert_eq!(frozen.blocked_sends, 1);
    tokio::time::advance(Duration::from_secs(60)).await;
    let held = session.pressure_for_test();
    assert_eq!(
        held.blocked_total,
        frozen.blocked_total + Duration::from_secs(60)
    );
    assert_eq!(held.blocked_max, held.blocked_total, "one wait, still open");
    assert_eq!(held.blocked_sends, 1);
    assert_eq!(
        held.end,
        ReaderEnd::Stopped,
        "sixty seconds on the channel is not the 45 second read limit"
    );
    assert!(!session.reader.as_ref().unwrap().is_finished());

    // Release: one slot frees and the waiting event goes in.
    assert!(session.next_event().await.is_some());
    let_the_reader_run().await;
    tokio::time::advance(Duration::from_secs(1)).await;
    let released = session.pressure_for_test();
    assert_eq!(released.blocked_total, held.blocked_total, "the wait ended");
    assert_eq!(released.blocked_max, held.blocked_total);
    assert_eq!(released.blocked_sends, 1);

    for _ in 0..GEMINI_EVENT_QUEUE {
        assert!(session.next_event().await.is_some());
    }
    let drained = session.pressure_for_test();
    assert_eq!((drained.received, drained.consumed), (257, 257));
    assert_eq!(
        drained.high_water, 256,
        "the high water is the peak, not what the channel holds now"
    );

    // A peer that goes quiet for the limit ends the reader as idle, and that
    // wait is not added to the blocked time.
    tokio::time::advance(READ_IDLE_LIMIT).await;
    assert!(session.next_event().await.is_none());
    let ended = session.pressure_for_test();
    assert_eq!(ended.end, ReaderEnd::ReadIdle);
    assert_eq!(ended.blocked_total, held.blocked_total);
    assert_eq!(ended.blocked_sends, 1);
    assert_eq!(ended.consumed, 257, "the close consumed nothing");
    assert_eq!(session.take_pressure(), Some(ended));
    assert_eq!(session.take_pressure(), None, "reported once");
}

#[tokio::test]
async fn cancelling_the_reader_mid_wait_closes_the_wait_where_it_stopped() {
    let mut session = flooded_session(GEMINI_EVENT_QUEUE + 1).await;
    tokio::time::pause();
    let frozen = session.pressure_for_test();
    tokio::time::advance(Duration::from_secs(60)).await;

    session.stop_reader().await;
    let stopped = session.pressure_for_test();
    assert_eq!(
        stopped.blocked_total,
        frozen.blocked_total + Duration::from_secs(60)
    );
    assert_eq!(stopped.blocked_max, stopped.blocked_total);
    assert_eq!((stopped.blocked_sends, stopped.high_water), (1, 256));
    assert_eq!((stopped.received, stopped.consumed), (257, 0));
    assert_eq!(stopped.end, ReaderEnd::Stopped);

    tokio::time::advance(Duration::from_secs(30)).await;
    assert_eq!(
        session.pressure_for_test(),
        stopped,
        "a cancelled wait does not keep growing"
    );
    assert_eq!(session.take_pressure(), Some(stopped));
    assert_eq!(session.take_pressure(), None);
}

#[tokio::test]
async fn dropping_a_session_mid_wait_closes_the_wait_where_it_stopped() {
    let session = flooded_session(GEMINI_EVENT_QUEUE + 1).await;
    let pressure = session.pressure.clone();
    tokio::time::pause();
    let frozen = pressure.peek();
    tokio::time::advance(Duration::from_secs(20)).await;
    drop(session);
    let_the_reader_run().await;
    let stopped = pressure.peek();
    assert_eq!(
        stopped.blocked_total,
        frozen.blocked_total + Duration::from_secs(20)
    );
    tokio::time::advance(Duration::from_secs(30)).await;
    assert_eq!(pressure.peek(), stopped);
}

#[tokio::test]
async fn a_silent_peer_ends_as_read_idle_without_blocked_time() {
    let config = live_config(&[]);
    let boot = bootstrap(&config, "interview-fixed", Some("two-sum"), 45);
    let (address, _frames) = stalled_live_server(false).await;
    let mut session = open_fixture_at(&format!("ws://{address}"), &boot, None)
        .await
        .unwrap();
    tokio::time::pause();
    tokio::time::advance(READ_IDLE_LIMIT).await;
    assert!(session.next_event().await.is_none());
    let idle = session.pressure_for_test();
    assert_eq!(idle.end, ReaderEnd::ReadIdle);
    assert_eq!(idle.blocked_sends, 0);
    assert_eq!(idle.blocked_total, Duration::ZERO);
    assert_eq!(idle.blocked_max, Duration::ZERO);
    assert_eq!((idle.received, idle.consumed, idle.high_water), (0, 0, 0));
    assert_eq!(idle.capacity, 256);
}

#[tokio::test]
async fn the_high_water_of_a_few_held_events_is_their_count() {
    let mut session = flooded_session(3).await;
    let held = session.pressure_for_test();
    assert_eq!((held.high_water, held.received, held.consumed), (3, 3, 0));
    assert_eq!(
        (held.blocked_sends, held.blocked_total),
        (0, Duration::ZERO)
    );
    for _ in 0..3 {
        assert!(session.next_event().await.is_some());
    }
    let taken = session.pressure_for_test();
    assert_eq!(
        (taken.high_water, taken.received, taken.consumed),
        (3, 3, 3)
    );
    assert_eq!(taken.blocked_sends, 0);
}

// Ordering of received versus consumed state behind a held consumer.
//
// The reader applies a frame's bookkeeping (resumption handle, busy evidence,
// usage ledger) when it decodes the frame, and only then queues the frame's
// events, so with a held consumer the received state runs ahead of the consumed
// one. Declared before running, for the mixed sequence below (events in queue
// order: Audio, OutputTranscript, ToolCall, UsageRecorded, Audio,
// UsageRecorded, TurnComplete, and a closing usage-only frame (23), whose
// arrival proves every earlier frame, the non-resumable update included, was
// applied):
//
// * with nothing consumed: 8 received, 0 consumed, high water 8, no blocked
//   send; the handle is the newest resumable one ("h2"; the later
//   non-resumable update must not replace it); three usage observations are on
//   the ledger; the socket is no longer busy, because the completion was read
//   even though the audio of that turn is still queued;
// * events come out in the order they arrived and a UsageRecorded event takes
//   the observation of its own frame, in order (21, 22, 23);
// * for every number of consumed events k, cancelling the reader and
//   draining the ledger yields exactly the observations of the UsageRecorded
//   events not yet consumed, so consumed plus drained is each observation
//   once: no loss and no repeat at any cut point;
// * the handle and the checkpoint age do not depend on k.
//
// Limits: one scripted frame order over loopback, no provider; it says nothing
// about what the provider does with a handle newer than the consumed events.

fn mixed_frames() -> Vec<String> {
    let audio = |data: &str| {
        format!(
            r#"{{"serverContent":{{"modelTurn":{{"parts":[{{"inlineData":{{"mimeType":"audio/pcm","data":"{data}"}}}}]}}}}}}"#
        )
    };
    vec![
        audio("AAAA"),
        r#"{"serverContent":{"outputTranscription":{"text":"so tell me"}}}"#.into(),
        format!(
            r#"{{"toolCall":{{"functionCalls":[{{"id":"call-q","name":"{}","args":{{}}}}]}}}}"#,
            crate::runtime::TOOL_LOG_HINT
        ),
        r#"{"usageMetadata":{"promptTokenCount":21}}"#.into(),
        r#"{"sessionResumptionUpdate":{"resumable":true,"newHandle":"h1"}}"#.into(),
        audio("BBBB"),
        r#"{"serverContent":{"turnComplete":true},"usageMetadata":{"promptTokenCount":22}}"#.into(),
        r#"{"sessionResumptionUpdate":{"resumable":true,"newHandle":"h2"}}"#.into(),
        r#"{"sessionResumptionUpdate":{"resumable":false,"newHandle":"not-resumable"}}"#.into(),
        r#"{"usageMetadata":{"promptTokenCount":23}}"#.into(),
    ]
}

/// A peer that completes setup, plays `frames` and goes quiet.
async fn scripted_frames_server(frames: Vec<String>) -> std::net::SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut socket = accept_async(socket).await.unwrap();
        let _ = socket.next().await;
        socket
            .send(Message::Text(r#"{"setupComplete":{}}"#.into()))
            .await
            .unwrap();
        for frame in frames {
            socket.send(Message::Text(frame.into())).await.unwrap();
        }
        std::future::pending::<()>().await;
    });
    address
}

async fn mixed_session() -> GeminiLiveSession {
    let config = live_config(&[]);
    let boot = bootstrap(&config, "interview-fixed", Some("two-sum"), 45);
    let address = scripted_frames_server(mixed_frames()).await;
    let session = open_fixture_at(&format!("ws://{address}"), &boot, None)
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while session.pressure_for_test().received < 8 {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .expect("the reader applies every frame while the consumer is held");
    session
}

fn expected_events() -> Vec<GeminiEvent> {
    let audio = |bytes: Vec<u8>| GeminiEvent::Audio {
        bytes,
        mime_type: "audio/pcm".into(),
    };
    vec![
        audio(vec![0, 0, 0]),
        GeminiEvent::OutputTranscript("so tell me".into()),
        GeminiEvent::ToolCall(vec![GeminiFunctionCall {
            id: "call-q".into(),
            name: crate::runtime::TOOL_LOG_HINT.into(),
            args: serde_json::json!({}),
        }]),
        GeminiEvent::UsageRecorded,
        audio(vec![0x04, 0x10, 0x41]),
        GeminiEvent::UsageRecorded,
        GeminiEvent::TurnComplete,
        GeminiEvent::UsageRecorded,
    ]
}

#[tokio::test]
async fn received_state_runs_ahead_of_a_held_consumer_without_reordering_events() {
    let mut session = mixed_session().await;
    let held = session.pressure_for_test();
    assert_eq!((held.received, held.consumed, held.high_water), (8, 0, 8));
    assert_eq!(held.blocked_sends, 0);
    assert_eq!(session.resumption_handle().as_deref(), Some("h2"));
    assert!(session.checkpoint_age().is_some());
    assert_eq!(pending_usage(&session), 3);
    assert!(
        !session.busy(),
        "the completion was read while its audio still sits in the queue"
    );

    let mut taken = Vec::new();
    let mut events = Vec::new();
    for _ in 0..8 {
        let event = session.next_event().await.unwrap();
        if event == GeminiEvent::UsageRecorded {
            taken.push(session.take_usage().unwrap().prompt);
        }
        events.push(event);
    }
    assert_eq!(
        taken,
        vec![21, 22, 23],
        "each completion takes its own frame's"
    );
    assert_eq!(events, expected_events());
    let done = session.pressure_for_test();
    assert_eq!((done.received, done.consumed), (8, 8));
    assert_eq!(pending_usage(&session), 0);
    assert_eq!(session.resumption_handle().as_deref(), Some("h2"));
}

#[tokio::test]
async fn every_cut_point_accounts_each_usage_observation_exactly_once() {
    for consumed in 0..=8usize {
        let mut session = mixed_session().await;
        let mut taken = Vec::new();
        for _ in 0..consumed {
            if session.next_event().await.unwrap() == GeminiEvent::UsageRecorded {
                taken.push(session.take_usage().unwrap().prompt);
            }
        }
        session.stop_reader().await;
        let drained = session
            .drain_usage()
            .into_iter()
            .map(|usage| usage.prompt)
            .collect::<Vec<_>>();
        let observed = session.pressure_for_test();
        assert_eq!((observed.received, observed.consumed), (8, consumed as u64));
        let mut all = taken.clone();
        all.extend(drained.iter().copied());
        assert_eq!(all, vec![21, 22, 23], "cut after {consumed} events");
        let unconsumed_usage = [3usize, 5, 7]
            .iter()
            .filter(|position| **position >= consumed)
            .count();
        assert_eq!(drained.len(), unconsumed_usage, "cut after {consumed}");
        assert!(session.take_usage().is_none(), "cut after {consumed}");
        assert_eq!(
            session.resumption_handle().as_deref(),
            Some("h2"),
            "the handle does not depend on what was consumed"
        );
        assert!(session.checkpoint_age().is_some(), "cut after {consumed}");
    }
}

// Pings behind a held consumer. Declared before running: while the reader is
// parked on a full channel it does not read the socket, so a provider ping
// written after the flood is not answered for as long as the wait lasts (60
// logical seconds here) and the peer sees no pong; the client's own writer is
// independent, so its keepalive ping still reaches the peer; the stall has no
// other local consequence: the reader is neither idle nor ended, the blocked
// counters are the only trace. When the consumer resumes and the reader reaches
// the ping, the pong goes out.
//
// Limits: loopback peer, tungstenite's automatic pong; it does not show what
// the real provider does with an unanswered ping, which was not observed.
#[tokio::test]
async fn a_reader_parked_on_the_channel_leaves_a_provider_ping_unanswered() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (seen, mut peer_received) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut socket = accept_async(socket).await.unwrap();
        let _ = socket.next().await;
        socket
            .send(Message::Text(r#"{"setupComplete":{}}"#.into()))
            .await
            .unwrap();
        let audio = r#"{"serverContent":{"modelTurn":{"parts":[{"inlineData":{"mimeType":"audio/pcm","data":"AAAA"}}]}}}"#;
        for _ in 0..=GEMINI_EVENT_QUEUE {
            socket.send(Message::Text(audio.into())).await.unwrap();
        }
        socket
            .send(Message::Ping(Default::default()))
            .await
            .unwrap();
        // Reading answers the client's ping with a pong on its own.
        while let Some(Ok(frame)) = socket.next().await {
            let _ = seen.send(frame);
        }
    });
    let config = live_config(&[]);
    let boot = bootstrap(&config, "interview-fixed", Some("two-sum"), 45);
    let mut session = open_fixture_at(&format!("ws://{address}"), &boot, None)
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while session.pressure_for_test().blocked_sends == 0 {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .expect("the reader fills the channel and parks");
    // Real time so far: give the peer's ping time to sit in the socket buffer.
    tokio::time::sleep(Duration::from_millis(100)).await;

    tokio::time::pause();
    tokio::time::advance(KEEPALIVE_INTERVAL + Duration::from_secs(1)).await;
    session.keep_alive().await.unwrap();
    tokio::time::advance(Duration::from_secs(60)).await;
    tokio::time::resume();
    tokio::time::sleep(Duration::from_millis(200)).await;
    let mut parked = Vec::new();
    while let Ok(frame) = peer_received.try_recv() {
        parked.push(frame);
    }
    assert!(
        parked.iter().any(|frame| frame.is_ping()),
        "the client's own keepalive still reaches the peer: {parked:?}"
    );
    assert!(
        parked.iter().all(|frame| !frame.is_pong()),
        "no pong while the reader is parked: {parked:?}"
    );
    let waiting = session.pressure_for_test();
    assert!(waiting.blocked_sends >= 1 && waiting.blocked_total >= Duration::from_secs(60));
    assert_eq!(waiting.end, ReaderEnd::Stopped);
    assert!(
        !session.reader.as_ref().unwrap().is_finished(),
        "a parked reader is neither idle nor ended"
    );

    for _ in 0..=GEMINI_EVENT_QUEUE {
        assert!(session.next_event().await.is_some());
    }
    let pong = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(frame) = peer_received.recv().await
                && frame.is_pong()
            {
                break frame;
            }
        }
    })
    .await
    .expect("the pong follows once the reader reaches the ping");
    assert!(pong.is_pong());
    assert_eq!(session.pressure_for_test().end, ReaderEnd::Stopped);
    assert!(!session.reader.as_ref().unwrap().is_finished());
}

fn pending_usage(session: &GeminiLiveSession) -> usize {
    session
        .usage
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .len()
}

// Sustained saturation, diagnosed from the pressure counters. Declared before
// running. The peer sends 600 frames: frame i is a turn end carrying usage i
// when i % 100 == 99 (6), else a transcript "t{i}" when i % 25 == 24 (18), else
// a tool call "c{i}" when i % 200 == 150 (3), else audio (573). That is 606
// events, 6 of them UsageRecorded.
// * a consumer that takes one event per 100 logical milliseconds (about 60
//   logical seconds in all) receives exactly those 606 events in frame order,
//   with the 6 usage frames taken in order, and consumes all 606: nothing is
//   dropped, merged or reordered, the reader is never idle or ended, and the
//   channel never holds more than its 256 slots (high water 256);
// * the reader parks on the full channel for most of the run (blocked time at
//   least 30 logical seconds, no more than the run), which is the only
//   trace; the same script against an immediate consumer yields the same
//   events and the same usage, so the slow consumer costs time and nothing
//   else, and the whole sequence equals the one the script decodes to, the
//   tool call ids and the position of every turn end included. Each event
//   waits at most 256 consumer periods, about 25 logical seconds, by
//   construction of the bound; that age is not measured here;
// * with the channel full and the reader parked, stopping the reader keeps the
//   256 queued events readable and the in-flight 257th is dropped with the
//   cancelled send; the ledger holds exactly the usage of the frames decoded
//   so far (99 and 199), and the four later ones were never read from the
//   socket, which is what letting a socket go means.
//
// Limits: one loopback peer and a scripted consumer rate. It shows no
// required-event loss or reordering from saturation; it does not show what the
// provider does when its client stops reading, which was not observed, and the
// consumer rate is chosen, not measured from a real room loop.

fn saturation_frames() -> Vec<String> {
    (0..600)
        .map(|i| match i {
            _ if i % 100 == 99 => format!(
                r#"{{"serverContent":{{"turnComplete":true}},"usageMetadata":{{"promptTokenCount":{i}}}}}"#
            ),
            _ if i % 25 == 24 => {
                format!(r#"{{"serverContent":{{"outputTranscription":{{"text":"t{i}"}}}}}}"#)
            }
            _ if i % 200 == 150 => format!(
                r#"{{"toolCall":{{"functionCalls":[{{"id":"c{i}","name":"{}","args":{{}}}}]}}}}"#,
                crate::runtime::TOOL_LOG_HINT
            ),
            _ => r#"{"serverContent":{"modelTurn":{"parts":[{"inlineData":{"mimeType":"audio/pcm","data":"AAAA"}}]}}}"#.to_string(),
        })
        .collect()
}

/// The events the script decodes to, frame by frame: a turn end is its usage
/// observation and then the completion.
fn saturation_expected_events() -> Vec<GeminiEvent> {
    let mut events = Vec::new();
    for i in 0..600 {
        match i {
            _ if i % 100 == 99 => {
                events.push(GeminiEvent::UsageRecorded);
                events.push(GeminiEvent::TurnComplete);
            }
            _ if i % 25 == 24 => events.push(GeminiEvent::OutputTranscript(format!("t{i}"))),
            _ if i % 200 == 150 => events.push(GeminiEvent::ToolCall(vec![GeminiFunctionCall {
                id: format!("c{i}"),
                name: crate::runtime::TOOL_LOG_HINT.into(),
                args: serde_json::json!({}),
            }])),
            _ => events.push(GeminiEvent::Audio {
                bytes: vec![0, 0, 0],
                mime_type: "audio/pcm".into(),
            }),
        }
    }
    events
}

async fn saturated_session() -> GeminiLiveSession {
    let config = live_config(&[]);
    let boot = bootstrap(&config, "interview-fixed", Some("two-sum"), 45);
    let address = scripted_frames_server(saturation_frames()).await;
    let session = open_fixture_at(&format!("ws://{address}"), &boot, None)
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while session.pressure_for_test().blocked_sends == 0 {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .expect("the reader fills the channel and parks");
    session
}

/// `total` events with the usage each UsageRecorded took, advancing `pace` of
/// logical time after each.
async fn take_all(
    session: &mut GeminiLiveSession,
    total: usize,
    pace: Duration,
) -> (Vec<GeminiEvent>, Vec<u64>) {
    let mut events = Vec::new();
    let mut usage = Vec::new();
    for _ in 0..total {
        let event = session.next_event().await.expect("an event for every slot");
        if event == GeminiEvent::UsageRecorded {
            usage.push(session.take_usage().unwrap().prompt);
        }
        events.push(event);
        if !pace.is_zero() {
            tokio::time::advance(pace).await;
        }
    }
    (events, usage)
}

#[tokio::test]
async fn sixty_seconds_of_slow_consumption_keeps_every_required_event_in_order() {
    let mut control = saturated_session().await;
    let (control_events, control_usage) = take_all(&mut control, 606, Duration::ZERO).await;
    let control_pressure = control.pressure_for_test();

    let mut slow = saturated_session().await;
    // Real time for the peer's last writes to settle before the clock stops.
    tokio::time::sleep(Duration::from_millis(100)).await;
    tokio::time::pause();
    let started = tokio::time::Instant::now();
    let (events, usage) = take_all(&mut slow, 606, Duration::from_millis(100)).await;
    let ran = started.elapsed();
    let pressure = slow.pressure_for_test();
    tokio::time::resume();

    assert!(
        ran >= Duration::from_secs(60) && ran < Duration::from_secs(61),
        "{ran:?}"
    );
    assert_eq!(
        events, control_events,
        "same events as an immediate consumer"
    );
    assert_eq!(events, saturation_expected_events());
    assert_eq!(usage, control_usage);
    assert_eq!(usage, vec![99, 199, 299, 399, 499, 599]);
    let count = |wanted: fn(&GeminiEvent) -> bool| events.iter().filter(|e| wanted(e)).count();
    assert_eq!(events.len(), 606);
    assert_eq!(count(|e| matches!(e, GeminiEvent::Audio { .. })), 573);
    assert_eq!(count(|e| matches!(e, GeminiEvent::OutputTranscript(_))), 18);
    assert_eq!(count(|e| matches!(e, GeminiEvent::ToolCall(_))), 3);
    assert_eq!(count(|e| *e == GeminiEvent::UsageRecorded), 6);
    assert_eq!(count(|e| *e == GeminiEvent::TurnComplete), 6);
    let transcripts: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            GeminiEvent::OutputTranscript(text) => Some(text.clone()),
            _ => None,
        })
        .collect();
    let wanted: Vec<_> = (0..600)
        .filter(|i| i % 25 == 24 && i % 100 != 99)
        .map(|i| format!("t{i}"))
        .collect();
    assert_eq!(transcripts, wanted);

    assert_eq!((pressure.received, pressure.consumed), (606, 606));
    assert_eq!(pressure.high_water, 256, "bounded by the channel");
    assert!(pressure.blocked_sends >= 1);
    assert!(
        pressure.blocked_total >= Duration::from_secs(30) && pressure.blocked_total <= ran,
        "{pressure:?}"
    );
    assert_eq!(pressure.end, ReaderEnd::Stopped, "never idle or ended");
    assert_eq!(
        (control_pressure.received, control_pressure.consumed),
        (606, 606)
    );
    assert!(!slow.reader.as_ref().unwrap().is_finished());
    assert_eq!(pending_usage(&slow), 0);
}

#[tokio::test]
async fn stopping_a_reader_parked_on_a_full_channel_accounts_usage_exactly() {
    let mut session = saturated_session().await;
    let parked = session.pressure_for_test();
    assert_eq!(
        (
            parked.received,
            parked.consumed,
            parked.high_water,
            parked.blocked_sends
        ),
        (257, 0, 256, 1)
    );
    session.stop_reader().await;
    let stopped = session.pressure_for_test();
    assert_eq!((stopped.received, stopped.consumed), (257, 0));
    let drained = session
        .drain_usage()
        .into_iter()
        .map(|usage| usage.prompt)
        .collect::<Vec<_>>();
    assert_eq!(drained, vec![99, 199]);
    let mut readable = 0;
    while session.next_event().await.is_some() {
        readable += 1;
    }
    assert_eq!(
        readable, 256,
        "the in-flight send is dropped with the reader"
    );
    assert!(session.next_event().await.is_none(), "ends, never waits");
}

/// Only a message that says setup is complete ends the wait, whether it
/// arrives as text or as a binary frame, and a frame that says anything else
/// is read past: stopping at the first one would open a session the provider
/// has not accepted.
#[tokio::test]
async fn only_a_setup_complete_message_ends_the_setup_wait() {
    use futures_util::stream;
    type Frame = Result<Message, tokio_tungstenite::tungstenite::Error>;
    let text = |body: &str| -> Frame { Ok(Message::Text(body.to_string().into())) };
    let binary = |body: &str| -> Frame { Ok(Message::Binary(body.as_bytes().to_vec().into())) };
    let other = r#"{"serverContent":{}}"#;
    let complete = r#"{"setupComplete":{}}"#;
    for frames in [
        vec![text(other), text(complete)],
        vec![binary(other), binary(complete)],
    ] {
        let mut stream = stream::iter(frames);
        super::setup::wait_for_setup_complete(&mut stream, &[])
            .await
            .unwrap();
        assert!(
            stream.next().await.is_none(),
            "the wait read past the frame that was not setupComplete"
        );
    }
}

/// Joins a fake provider's task, failing the test when the client never gave
/// it what it waits for. Left unbounded, a client that stops sending hangs the
/// suite instead of failing the test, and a hung suite cannot report anything.
async fn bounded<T>(task: tokio::task::JoinHandle<T>) -> T {
    tokio::time::timeout(Duration::from_secs(10), task)
        .await
        .expect("the fake provider was left waiting for the client")
        .unwrap()
}

/// Ordinary fixtures require an immediate setup reply. Waiting for the full
/// provider deadline in each test serializes failures on a small CI runner.
/// Tests of the exchange deadline call the production opener directly.
async fn open_fixture_at(
    url: &str,
    boot: &dyn LiveSession,
    resume: Option<&str>,
) -> Result<GeminiLiveSession, Box<dyn std::error::Error + Send + Sync>> {
    tokio::time::timeout(
        Duration::from_secs(2),
        super::open_live_session_at(url, boot, resume),
    )
    .await
    .expect("the fixture provider must complete setup")
}
