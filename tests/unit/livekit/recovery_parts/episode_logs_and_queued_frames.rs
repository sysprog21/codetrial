// Part of recovery.rs: included in place, so every item here shares that file's
// module.

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
            entered: std::sync::Arc::clone(&entered),
            release: std::sync::Arc::clone(&open),
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
        rig.gemini = crate::livekit::tests::fixture_session_at(&endpoint, &rig.keys, &boot, None)
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

// Candidate-audio stores, characterized separately. The SDK queue is in
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
