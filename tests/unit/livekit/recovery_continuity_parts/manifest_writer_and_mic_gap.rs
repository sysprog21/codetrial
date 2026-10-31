// Part of recovery_continuity.rs: included in place, so every item here shares
// that file's module.

/// Declared before running: the manifest writer, started as the launcher
/// wrapper starts it, writes for each arm a clip manifest the controller
/// accepts with its binary pin verified, naming the two planned actions and
/// the clip set's own digest.
#[test]
fn the_manifest_writer_produces_each_arms_clip_manifest() {
    let scratch = Scratch::new("arm-writer");
    let clips = write_clip_dir(&scratch, |_| {});
    let name = format!(
        "{}::live_write_manifest",
        module_path!()
            .split_once("::")
            .unwrap()
            .1
            .rsplit_once("::continuity")
            .unwrap()
            .0
    );
    for arm in &ARMS {
        let out = scratch.path(&format!("manifest-{}.json", arm.id));
        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &name, "--nocapture"])
            .env("CODETRIAL_LIVE_MANIFEST_OUT", &out)
            .env("CODETRIAL_LIVE_SCENARIO", arm.id)
            .env("CODETRIAL_LIVE_CANDIDATE", std::env::current_exe().unwrap())
            .env("CODETRIAL_LIVE_CLIPS", &clips)
            .output()
            .unwrap();
        assert!(
            child.status.success(),
            "{}",
            String::from_utf8_lossy(&child.stderr)
        );
        let manifest: Value = serde_json::from_slice(&std::fs::read(&out).unwrap()).unwrap();
        assert_eq!(manifest["run_id"], json!(arm.run_id()));
        assert_eq!(manifest["actions"], json!(CLIP_SCENARIO.actions));
        assert_eq!(
            manifest["fixture_sha256"],
            json!(clips_fixture_sha(Some(clips.to_str().unwrap().to_owned())))
        );
        let checked = std::process::Command::new("python3")
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/interview-run-budget.py"))
            .args(["validate", "--verify-binaries", "--manifest"])
            .arg(&out)
            .output()
            .unwrap();
        assert!(
            checked.status.success(),
            "{}: {}{}",
            arm.id,
            String::from_utf8_lossy(&checked.stdout),
            String::from_utf8_lossy(&checked.stderr)
        );
    }
}

fn clean_teardown() -> Teardown {
    Teardown {
        participants: Some(Vec::new()),
        room_gone_after_delete: true,
        readers_unfinished: 0,
        notes: Vec::new(),
    }
}

/// Declared before running: an arm that held its gate, saw the consumer reach
/// it, took frames after the release, published every planned frame and left a
/// clean room passes; each departure is named, the control's gate is never
/// held, and a transcript is never part of the verdict.
#[test]
fn the_arm_verdict_names_each_departure_and_ignores_the_transcript() {
    let arm = &ARMS[2];
    let mut good = observation(arm, &[]);
    good.frames_after_release = 5;
    assert!(judge_arm(arm, Some(&good), &clean_teardown(), &Ok(())).is_empty());
    let mut bad = good;
    bad.frames_published = 10;
    bad.complete = false;
    bad.held_arrivals = 0;
    bad.frames_after_release = 0;
    let problems = judge_arm(arm, Some(&bad), &clean_teardown(), &Ok(()));
    assert_eq!(problems.len(), 3, "{problems:?}");
    let dirty = Teardown {
        participants: Some(vec!["candidate-lives".into()]),
        room_gone_after_delete: false,
        readers_unfinished: 2,
        notes: Vec::new(),
    };
    let problems = judge_arm(
        arm,
        None,
        &dirty,
        &Err(ScriptError::Permit("denied".into())),
    );
    assert_eq!(problems.len(), 5, "{problems:?}");
    let mut control = observation(&ARMS[0], &FULL);
    assert!(judge_arm(&ARMS[0], Some(&control), &clean_teardown(), &Ok(())).is_empty());
    control.held_ms = Some(3);
    assert_eq!(
        judge_arm(&ARMS[0], Some(&control), &clean_teardown(), &Ok(())).len(),
        1
    );
}

/// Loads the spoken clips when `CODETRIAL_LIVE_CLIPS` names them. It reads
/// local files and nothing else: the check that the authored answer key is the
/// text of the real clips.
#[test]
fn the_spoken_clips_load_when_their_directory_is_named() {
    let Ok(dir) = std::env::var("CODETRIAL_LIVE_CLIPS") else {
        return;
    };
    let set = ClipSet::load(Path::new(&dir)).unwrap();
    let timeline = Timeline::new(&set);
    println!(
        "clips {:?} frames {} spans {:?}",
        CLIP_IDS,
        timeline.clip_frames(),
        timeline.spans
    );
    for arm in &ARMS {
        Schedule::new(arm, timeline.clip_frames())
            .check_budget()
            .unwrap();
    }
}

/// Declared before running: an arm that did not complete is inconclusive in
/// every finding, never a loss; a control that did not complete, or whose gate
/// was held, is no baseline, so a stalled arm's findings are inconclusive for
/// want of one.
#[test]
fn an_incomplete_arm_or_control_never_turns_a_miss_into_a_loss() {
    let mut cut = observation(&ARMS[1], &FULL[3..]);
    cut.complete = false;
    let reports = compare(&[observation(&ARMS[0], &FULL), cut]);
    assert_eq!(reports[1].findings(Kind::Missing), 0);
    assert_eq!(reports[1].inconclusive(), 3);
    assert_eq!(
        statuses(&reports[1])[0],
        [Status::Inconclusive(
            Kind::Missing,
            "the arm did not complete"
        )]
    );

    let whole = observation(&ARMS[1], &FULL[3..]);
    let mut bad_control = observation(&ARMS[0], &FULL);
    bad_control.complete = false;
    let reports = compare(&[bad_control.clone(), whole.clone()]);
    assert_eq!(reports[1].findings(Kind::Missing), 0);
    assert_eq!(reports[1].inconclusive(), 3);
    bad_control.complete = true;
    bad_control.held_ms = Some(0);
    assert_eq!(compare(&[bad_control, whole])[1].findings(Kind::Missing), 0);
}

// The SDK's overflow log, offline.

fn captured(at: Instant, target: &str, message: &str) -> overflow_log::Captured {
    overflow_log::Captured {
        at,
        target: target.to_owned(),
        message: message.to_owned(),
    }
}

fn feed(capture: &overflow_log::Capture, level: log::Level, target: &str, message: &str) {
    use log::Log as _;
    capture.log(
        &log::Record::builder()
            .level(level)
            .target(target)
            .args(format_args!("{message}"))
            .build(),
    );
}

const SDK_TARGET: &str = "libwebrtc::native::audio_stream";
const SDK_LINE: &str = "native audio stream queue overflow; dropped 100 queued frames";

/// Declared before running: records from the SDK's audio-stream targets that
/// mention overflow or dropped frames are kept in order with their time and
/// target; the video stream's line, an unrelated SDK line, an application
/// target and an info-level line are not; the store holds at most 64 lines,
/// each
/// target at most 64 characters and each message at most 160, and counts the
/// lines it could not store; a registered secret in a message is redacted.
#[test]
fn the_overflow_logger_keeps_only_bounded_audio_stream_overflow_lines() {
    let capture = overflow_log::Capture::new();
    feed(&capture, log::Level::Warn, SDK_TARGET, SDK_LINE);
    feed(
        &capture,
        log::Level::Warn,
        "livekit::rtc_engine",
        "audio frames dropped: 3",
    );
    for (level, target, message) in [
        (
            log::Level::Warn,
            "libwebrtc::native::video_stream",
            "native video stream queue overflow; dropped 100 queued frames",
        ),
        (log::Level::Warn, SDK_TARGET, "native audio stream started"),
        (log::Level::Warn, "codetrial::livekit", SDK_LINE),
        (log::Level::Info, SDK_TARGET, SDK_LINE),
    ] {
        feed(&capture, level, target, message);
    }
    let snapshot = capture.snapshot(true);
    assert_eq!(snapshot.records.len(), 2);
    assert_eq!(snapshot.records[0].target, SDK_TARGET);
    assert_eq!(snapshot.records[0].message, SDK_LINE);
    assert_eq!(snapshot.records[1].target, "livekit::rtc_engine");
    assert!(snapshot.records[0].at <= snapshot.records[1].at);
    assert_eq!(dropped_count(SDK_LINE), Some(100));
    assert_eq!(dropped_count("no count here"), None);

    // Bounded in number, target length and message length.
    let capture = overflow_log::Capture::new();
    let long = format!("{SDK_LINE} {}", "x".repeat(500));
    let long_target = format!("libwebrtc::{}", "t".repeat(200));
    for _ in 0..(overflow_log::MAX_RECORDS + 5) {
        feed(&capture, log::Level::Warn, &long_target, &long);
    }
    let snapshot = capture.snapshot(true);
    assert_eq!(snapshot.records.len(), overflow_log::MAX_RECORDS);
    assert_eq!(snapshot.unstored, 5);
    assert!(snapshot.records.iter().all(|r| {
        r.message.chars().count() <= overflow_log::MAX_MESSAGE
            && r.target.chars().count() <= overflow_log::MAX_TARGET
    }));
    capture.clear();
    assert!(capture.snapshot(true).records.is_empty());

    // A secret the process registered never reaches the record.
    register_secrets(["overflow-test-secret-value".to_owned()]);
    let capture = overflow_log::Capture::new();
    feed(
        &capture,
        log::Level::Warn,
        SDK_TARGET,
        "native audio stream queue overflow; dropped 1 queued frames overflow-test-secret-value",
    );
    let snapshot = capture.snapshot(true);
    assert!(
        !snapshot.records[0]
            .message
            .contains("overflow-test-secret-value")
    );
}

/// Declared before running: installing is idempotent and answers the same every
/// time; when this logger is the process's logger a `log::warn!` from an
/// SDK-shaped target reaches it through the `log` facade while a macro call
/// with another target does not. Other tests in the process never installed a
/// logger and are unaffected.
#[test]
fn installing_the_overflow_logger_twice_is_harmless_and_reaches_the_facade() {
    let first = overflow_log::install();
    assert_eq!(overflow_log::install(), first);
    assert_eq!(overflow_log::snapshot().installed, first);
    assert!(first, "no other test in this process installs a logger");
    {
        log::warn!(
            target: "libwebrtc::native::audio_stream",
            "native audio stream queue overflow; dropped 7777001 queued frames"
        );
        log::warn!(
            target: "codetrial::other",
            "native audio stream queue overflow; dropped 7777002 queued frames"
        );
        let seen: Vec<Option<u64>> = overflow_log::snapshot()
            .records
            .iter()
            .map(|r| dropped_count(&r.message))
            .collect();
        assert!(seen.contains(&Some(7_777_001)));
        assert!(!seen.contains(&Some(7_777_002)));
        overflow_log::CAPTURE.clear();
    }
}

/// Declared before running: a record is placed on the clip's clock, not the
/// probe's: with the clip beginning 300 ms after the probe and lines logged 500
/// and 900 ms after the probe, the observation says 200 and 600 ms and says the
/// logger was installed; the lines survive the arm_result round trip.
#[test]
fn overflow_lines_are_timestamped_from_the_clips_first_frame() {
    let probe = ContinuityProbe::new();
    let began = probe.epoch + Duration::from_millis(300);
    let arm = &ARMS[3];
    let schedule = Schedule::new(arm, 100);
    let published = Published {
        clip_frames: schedule.total_frames,
        began: Some(began),
        clips: Vec::new(),
        error: None,
    };
    let snapshot = overflow_log::Snapshot {
        installed: true,
        records: vec![
            captured(
                probe.epoch + Duration::from_millis(100),
                SDK_TARGET,
                SDK_LINE,
            ),
            captured(
                probe.epoch + Duration::from_millis(500),
                SDK_TARGET,
                SDK_LINE,
            ),
            captured(
                probe.epoch + Duration::from_millis(900),
                SDK_TARGET,
                "native audio stream queue overflow; dropped 200 queued frames",
            ),
        ],
        unstored: 0,
    };
    let built = Observation::build(arm, &schedule, &published, &probe, &snapshot);
    assert_eq!(
        built
            .sdk_overflow_logs
            .iter()
            .map(|line| (line.at_ms, line.dropped))
            .collect::<Vec<_>>(),
        [(-200, Some(100)), (200, Some(100)), (600, Some(200))]
    );
    assert!(built.sdk_overflow_logger);
    let line = format!(
        "LIVE_OBS {}\n",
        json!({"kind": "arm_result", "fields": built.to_json()})
    );
    assert_eq!(parse_arm_results(&line), [built]);
}

/// Declared before running: the report prints each overflow line with its time
/// and count and says a count within 99 and a time is all it is, not frame
/// identity; an arm with no line says the first drop always logs; an arm whose
/// logger was not installed concludes nothing; stored-line overflow is said.
#[test]
fn the_report_prints_overflow_lines_and_says_they_are_not_frame_identity() {
    let mut hit = observation(&ARMS[3], &FULL);
    hit.sdk_overflow_logs = vec![
        OverflowLog {
            at_ms: 4_100,
            dropped: Some(1),
            target: SDK_TARGET.into(),
            message: "native audio stream queue overflow; dropped 1 queued frames".into(),
        },
        OverflowLog {
            at_ms: 4_200,
            dropped: Some(100),
            target: SDK_TARGET.into(),
            message: SDK_LINE.into(),
        },
    ];
    hit.sdk_overflow_unstored = 3;
    let clear = observation(&ARMS[0], &FULL);
    let mut blind = observation(&ARMS[1], &FULL);
    blind.sdk_overflow_logger = false;
    let report = render_report(&[clear, blind, hit]);
    assert!(report.contains("4100 ms  dropped=Some(1)"));
    assert!(report.contains("4200 ms  dropped=Some(100)"));
    assert!(report.contains("highest count logged by any audio stream is Some(100)"));
    assert!(report.contains("a count within 99"));
    assert!(report.contains("frame identity is not known"));
    assert!(report.contains("not which frames were lost"));
    assert!(report.contains("no matching line was captured"));
    assert!(report.contains("was not the process's logger"));
    assert!(report.contains("3 further matching line(s) were not stored"));
    assert_eq!(report.matches("SDK overflow log (").count(), 3);
}

/// Declared before running: the real room-loop dequeue behind the
/// speech-continuity gate.
/// 100 frames are taken at once (so no gap above a few hundred ms), the gate
/// is then held for 1300 ms while 100 more are captured and the loop's timer
/// arm keeps running, and then released. The interview's measured maximum gap
/// is at least the hold and well under it plus a second of scheduling slack,
/// (a consumer parked on the stream may take the one frame that arrives right
/// after the hold starts, so the lower bound allows 100 ms) exactly one gap is
/// over 1 s, none over 10 s or 30 s, and the same figures
/// are on the next `interview clock:` line. A control with no hold measures
/// no gap over 1 s.
#[tokio::test]
async fn the_measured_microphone_gap_follows_the_held_dequeue_gate() {
    if isolated("live::continuity::the_measured_microphone_gap_follows_the_held_dequeue_gate") {
        return;
    }
    const HOLD: Duration = Duration::from_millis(1300);
    for (label, hold) in [("control", false), ("held", true)] {
        let probe = ContinuityProbe::new();
        let _installed = probe::install(&probe);
        let (peer, endpoint) = listener().await;
        let wire = Arc::new(std::sync::Mutex::new(Vec::new()));
        let server = recording_peer(peer, std::sync::Arc::clone(&wire));
        let mut rig = Rig::new("ws://127.0.0.1:9/".into()).await;
        let boot = crate::runtime::bootstrap(&rig.config, "recovery-test", Some("two-sum"), 45);
        let session = crate::livekit::tests::fixture_session_at(&endpoint, &rig.keys, &boot, None)
            .await
            .unwrap();
        let mut first = std::mem::replace(&mut rig.gemini, session);
        first.shutdown().await.unwrap();
        rig.old.take().unwrap().await.unwrap();
        let began = Instant::now();
        rig.turn.activity.latency.anchor(began);

        let source = NativeAudioSource::new(AudioSourceOptions::default(), SAMPLE_RATE, 1, 0);
        let track = LocalAudioTrack::create_audio_track(
            "continuity-gap",
            RtcAudioSource::Native(source.clone()),
        );
        rig.media.audio = Some(crate::livekit::media::candidate_audio_stream(
            track.rtc_track(),
        ));
        for marker in 1..=100i16 {
            source
                .capture(&numbered_frame(marker, FRAME_SAMPLES).data)
                .await
                .unwrap();
        }

        let mut ticker = tokio::time::interval(Duration::from_millis(10));
        let ended = tokio::time::Instant::now() + Duration::from_secs(20);
        let mut held_since = None;
        let mut captured_more = false;
        let mut controls = 0usize;
        loop {
            tokio::select! {
                input = crate::livekit::media::next_candidate_input(
                    &mut rig.media.audio,
                    &mut rig.media.recovery_audio,
                ), if rig.media.audio.is_some() => {
                    let mut context = rig.turn.context(
                        &mut rig.output, &mut rig.gemini, &mut rig.board, &mut rig.media,
                    );
                    crate::livekit::on_candidate_input(&mut context, input, 1500).await;
                }
                _ = ticker.tick() => {
                    controls += 1;
                    if held_since.is_none() && probe.gate.stats().frames >= 100 {
                        if hold {
                            probe.gate.hold();
                        }
                        held_since = Some(Instant::now());
                    }
                    if let Some(since) = held_since {
                        if !captured_more {
                            captured_more = true;
                            for marker in 101..=200i16 {
                                source
                                    .capture(&numbered_frame(marker, FRAME_SAMPLES).data)
                                    .await
                                    .unwrap();
                            }
                        }
                        if hold && since.elapsed() >= HOLD && probe.gate.is_held() {
                            probe.gate.release();
                        }
                        if probe.gate.stats().frames >= 200 {
                            break;
                        }
                    }
                    assert!(tokio::time::Instant::now() < ended, "{label}: never drained");
                }
            }
        }
        assert!(!hold || controls >= 100, "{label}: other arms kept running");
        let gaps = rig.turn.activity.latency.mic_gaps().fields();
        let field = |line: &str, name: &str| -> String {
            line.split_whitespace()
                .find_map(|token| token.strip_prefix(&format!("{name}=")))
                .unwrap()
                .to_string()
        };
        let max: u64 = field(&gaps, "mic_gap_max_ms").parse().unwrap();
        if hold {
            assert!(
                (1200..=2300).contains(&max),
                "{label}: max gap {max} ms vs a 1300 ms hold"
            );
            assert_eq!(field(&gaps, "mic_gap_over_1s"), "1", "{label}");
        } else {
            assert!(max < 1000, "{label}: max gap {max} ms");
            assert_eq!(field(&gaps, "mic_gap_over_1s"), "0", "{label}");
        }
        assert_eq!(field(&gaps, "mic_gap_over_10s"), "0", "{label}");
        assert_eq!(field(&gaps, "mic_gap_over_30s"), "0", "{label}");

        // The same figures reach the next clock line, with no new sample
        // needed: the line a minute later carries them.
        let line = rig
            .turn
            .activity
            .latency
            .clock_sample(began + Duration::from_secs(61))
            .unwrap();
        assert_eq!(field(&line, "mic_gap_max_ms"), max.to_string(), "{label}");
        rig.gemini.shutdown().await.unwrap();
        server.await.unwrap();
    }
}
