// Part of recovery_continuity.rs: included in place, so every item here shares
// that file's module.

/// Reads every `worker-s*.log` in a directory and writes the report beside
/// them. Does nothing unless the launcher wrapper asks, like the manifest
/// writer; it touches no provider.
#[test]
fn live_clip_report() {
    let Ok(dir) = std::env::var("CODETRIAL_LIVE_REPORT_DIR") else {
        return;
    };
    let mut observations = Vec::new();
    for arm in &ARMS {
        let log = std::fs::read_to_string(Path::new(&dir).join(format!("worker-{}.log", arm.id)))
            .unwrap_or_default();
        observations.extend(
            parse_arm_results(&log)
                .into_iter()
                .filter(|item| item.arm == arm.id),
        );
    }
    let report = render_report(&observations);
    std::fs::write(Path::new(&dir).join("continuity-report.txt"), &report).unwrap();
    println!("{report}");
}

// The live arm. NOT RUN.

/// The shared room fixture with an arm's own pieces.
struct ClipEffects<'a> {
    live: LiveEffects<'a>,
    arm: &'static Arm,
    timeline: Timeline,
    schedule: Schedule,
    probe: Arc<ContinuityProbe>,
    observation: Option<Observation>,
    published_error: Option<String>,
}

impl<'a> ClipEffects<'a> {
    fn new(
        arm: &'static Arm,
        clips: &ClipSet,
        probe: Arc<ContinuityProbe>,
        mut live: LiveEffects<'a>,
    ) -> Self {
        live.setup_s = CLIP_SETUP_S;
        let timeline = Timeline::new(clips);
        let schedule = Schedule::new(arm, timeline.clip_frames());
        Self {
            live,
            arm,
            timeline,
            schedule,
            probe,
            observation: None,
            published_error: None,
        }
    }

    /// Publishes the track, then runs the arm's clock to its end. Reads and
    /// waits here are not actions; the permit for `clip_start` covers the
    /// audio that leaves the candidate.
    async fn run_clip(&mut self) -> Result<(), String> {
        let link = self
            .live
            .link
            .as_ref()
            .ok_or("the candidate is not connected")?;
        let source = NativeAudioSource::new(AudioSourceOptions::default(), SAMPLE_RATE, 1, 100);
        let track = LocalAudioTrack::create_audio_track(
            "candidate-clip",
            RtcAudioSource::Native(source.clone()),
        );
        link.room
            .local_participant()
            .publish_track(
                LocalTrack::Audio(track),
                TrackPublishOptions {
                    source: TrackSource::Microphone,
                    ..Default::default()
                },
            )
            .await
            .map_err(|error| redact(&error.to_string()))?;
        emit(
            "clip_track_published",
            json!({"arm": self.arm.id, "clips": CLIP_IDS, "turns": ANSWER_KEY.iter().map(Turn::id).collect::<Vec<_>>()}),
        );
        let stop_file = self.live.stop_file.clone();
        let script_stop = self.live.script_stop;
        let live = &self.live;
        let probe = &self.probe;
        let emitted = std::cell::Cell::new(0usize);
        let overflow_emitted = std::cell::Cell::new(0usize);
        let began_at = std::cell::Cell::new(None::<Instant>);
        let published = run_publisher(
            &source,
            &self.timeline,
            &self.schedule,
            &probe.gate,
            || probe.gate.stats().frames >= SUBSCRIBED_FRAMES,
            || {
                if stop_file.as_deref().is_some_and(Path::exists) {
                    Some("stopped by the launcher".to_owned())
                } else if Instant::now() >= script_stop {
                    Some("the outer wall cap came during the arm".to_owned())
                } else if !live.agent_running() {
                    Some("the interviewer ended during the arm".to_owned())
                } else {
                    emit_new_transcripts(probe, began_at.get(), &emitted);
                    emit_new_overflows(began_at.get(), &overflow_emitted);
                    None
                }
            },
            |began| {
                probe.gate.rebase(began);
                began_at.set(Some(began));
                emit("clip_began", json!({"arm": self.arm.id}));
            },
        )
        .await;
        emit_new_transcripts(probe, published.began, &emitted);
        emit_new_overflows(published.began, &overflow_emitted);
        self.observation = Some(Observation::build(
            self.arm,
            &self.schedule,
            &published,
            probe,
            &overflow_log::snapshot(),
        ));
        match published.error {
            None => Ok(()),
            Some(error) => {
                self.published_error = Some(error.clone());
                Err(error)
            }
        }
    }

    /// Closes what the fixture owns, directly: the candidate's connection, then
    /// the interviewer's task. No End is sent, so no assessment report is
    /// generated or paid for.
    async fn close_owned(&mut self) -> Result<(), String> {
        self.probe.gate.release();
        let mut errors = Vec::new();
        if let Some(link) = self.live.link.take() {
            if let Err(error) = link.room.close().await {
                errors.push(redact(&error.to_string()));
            }
            link.observer.abort();
        }
        if let Some(agent) = self.live.agent.take() {
            agent.abort();
            self.live.agent_end = Some(AgentEnd::Aborted);
        }
        emit("owned_closed", json!({"errors": errors}));
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("; "))
        }
    }
}

/// Emits the overflow lines not yet emitted, on the arm's clock.
fn emit_new_overflows(began: Option<Instant>, emitted: &std::cell::Cell<usize>) {
    let snapshot = overflow_log::snapshot();
    let lines = &snapshot.records;
    for record in &lines[emitted.get().min(lines.len())..] {
        let at = began.map(|began| OverflowLog::at(record, began));
        emit(
            "sdk_overflow_log",
            json!({
                "at_ms": at.as_ref().map(|line| line.at_ms),
                "dropped": at.as_ref().and_then(|line| line.dropped),
                "target": record.target,
                "message": record.message,
            }),
        );
    }
    emitted.set(lines.len());
}

fn emit_new_transcripts(
    probe: &ContinuityProbe,
    began: Option<Instant>,
    emitted: &std::cell::Cell<usize>,
) {
    let lines = probe.transcripts();
    for line in &lines[emitted.get().min(lines.len())..] {
        let at = began.map(|began| line.at.saturating_duration_since(began).as_millis() as u64);
        emit(
            "transcript_line",
            json!({"at_ms": at, "text": redact(&line.text)}),
        );
    }
    emitted.set(lines.len());
}

impl Effects for ClipEffects<'_> {
    async fn perform(&mut self, action: &str) -> Result<(), String> {
        emit("action", json!({"action": action}));
        match action {
            "candidate_join" => self.live.join().await,
            "clip_start" => self.run_clip().await,
            FINAL_ACTION_ID => self.close_owned().await,
            other => Err(format!("{other} has no clip effect")),
        }
    }

    async fn await_gate_entered(&mut self) -> Result<(), String> {
        Ok(())
    }

    async fn await_terminal(&mut self, _: &Scenario) -> Result<(), String> {
        Ok(())
    }

    async fn release_gate(&mut self) {
        self.probe.gate.release();
    }
}

/// The problems an arm's own run shows. The transcript is not judged here: it
/// is the evidence, compared in the report.
fn judge_arm(
    arm: &Arm,
    observation: Option<&Observation>,
    teardown: &Teardown,
    script: &Result<(), ScriptError>,
) -> Vec<String> {
    let mut problems = Vec::new();
    if let Err(error) = script {
        problems.push(format!("the script did not complete: {error:?}"));
    }
    match observation {
        None => problems.push("the arm produced no observation".to_owned()),
        Some(observation) => {
            if !observation.complete {
                problems.push(format!(
                    "the arm did not complete: published {} of {} planned frames",
                    observation.frames_published, observation.frames_planned
                ));
            }
            if !arm.is_control() {
                if observation.held_arrivals == 0 {
                    problems.push("the consumer never reached the held gate".to_owned());
                }
                if observation.released_ms.is_none() || observation.frames_after_release == 0 {
                    problems.push("no frame was taken after the release".to_owned());
                }
            } else if observation.held_ms.is_some() {
                problems.push("the control's gate was held".to_owned());
            }
        }
    }

    // The interviewer runs in this process and is aborted rather than ended (an
    // End would produce a report, which this arm must not). LiveKit keeps
    // listing an aborted participant until the room is deleted, so it is
    // reported but not a leak; any other participant, or a room still listed
    // after deletion, is.
    if let Some(left) = &teardown.participants {
        let (aborted, others): (Vec<_>, Vec<_>) = left
            .iter()
            .partition(|identity| identity.starts_with("interviewer-"));
        if !aborted.is_empty() {
            eprintln!("aborted interviewer listed until the room was deleted: {aborted:?}");
        }
        if !others.is_empty() {
            problems.push(format!("participants left in the room: {others:?}"));
        }
    }
    if !teardown.room_gone_after_delete {
        problems.push("the room was still listed after it was deleted".into());
    }
    if teardown.readers_unfinished != 0 {
        problems.push(format!(
            "{} Live reader tasks were still running",
            teardown.readers_unfinished
        ));
    }
    problems
}

/// NOT RUN against LiveKit or Gemini. One arm per invocation, chosen by
/// `CODETRIAL_LIVE_SCENARIO` (s0, s20, s39, s41 or s60), under the B0.1
/// launcher with the clip manifest `live_write_manifest` writes: planned
/// actions `candidate_join` and `clip_start`, plus the reserved close permit.
/// It needs `CODETRIAL_LIVE_CONFIG` and `CODETRIAL_LIVE_CLIPS`, the directory
/// of
/// the spoken clips outside the repository.
///
/// Real here: the LiveKit room and SDK transport on both ends, an SDK audio
/// track published by the candidate participant, the interviewer's room loop,
/// its microphone dequeue and Gemini Live. Not real: the interview's end (the
/// fixture closes its own participants and sends no End, so no assessment
/// report is generated and no end or report behavior is certified) and the
/// transcript's fidelity, which is Gemini's speech-to-text.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "NOT RUN: spends LiveKit and Gemini quota; needs the B0.1 launcher variables, CODETRIAL_LIVE_CONFIG, CODETRIAL_LIVE_CLIPS, CODETRIAL_LIVE_SCENARIO and budget approval"]
async fn live_sdk_clip_continuity_arm() {
    let mut permits = PermitClient::from_env()
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    let id = std::env::var("CODETRIAL_LIVE_SCENARIO")
        .expect("set CODETRIAL_LIVE_SCENARIO to s0, s20, s39, s41 or s60");
    let arm = arm_for(&id).expect("CODETRIAL_LIVE_SCENARIO is one of s0, s20, s39, s41, s60");
    assert_eq!(permits.manifest["run_id"], json!(arm.run_id()));
    assert_eq!(permits.manifest["actions"], json!(CLIP_SCENARIO.actions));
    assert_eq!(permits.manifest["action_ceiling"], json!(3));
    assert_eq!(permits.manifest["outer_wall_cap_s"], json!(CLIP_CAP_S));
    assert_eq!(permits.manifest["end_mode"], json!("native_close"));
    assert_eq!(permits.manifest["run_kind"], json!("clip"));
    let clips_dir = std::env::var("CODETRIAL_LIVE_CLIPS").expect("set CODETRIAL_LIVE_CLIPS");
    assert_eq!(
        permits.manifest["fixture_sha256"],
        json!(clips_fixture_sha(Some(clips_dir.clone()))),
        "the clip set is not the one the manifest was written for"
    );
    let clips = ClipSet::load(Path::new(&clips_dir)).unwrap_or_else(|error| panic!("{error}"));
    let timeline = Timeline::new(&clips);
    Schedule::new(arm, timeline.clip_frames())
        .check_budget()
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(clips.frames(), timeline.clip_frames());

    let recording_cap = std::env::var("CODETRIAL_RECORDING_MAX_MINUTES")
        .ok()
        .map(|raw| raw.parse::<u32>().expect("a whole number of minutes"));
    let duration = clip_duration_min(&permits.manifest, recording_cap)
        .unwrap_or_else(|error| panic!("{error}"));
    let config = crate::config::load_from_pairs(
        crate::config::read_config_file(Path::new(
            &std::env::var("CODETRIAL_LIVE_CONFIG").expect("set CODETRIAL_LIVE_CONFIG"),
        ))
        .expect("read the isolated live config"),
    )
    .unwrap_or_else(|_| panic!("invalid isolated live config"));
    assert!(
        !config.google_api_keys.is_empty(),
        "the live config names no Gemini key"
    );
    register_secrets(config.google_api_keys.iter().cloned().chain([
        config.livekit_api_key.clone(),
        config.livekit_api_secret.clone(),
    ]));

    let unarmed = RecoveryGate {
        entered: Arc::new(tokio::sync::Notify::new()),
        release: Arc::new(tokio::sync::Notify::new()),
    };
    let probe = ContinuityProbe::new();
    let _installed = probe::install(&probe);
    overflow_log::install();
    overflow_log::CAPTURE.clear();
    let live = LiveEffects::new(
        &CLIP_SCENARIO,
        config,
        recording_cap,
        duration,
        &unarmed,
        permits.stop_file.clone(),
    )
    .unwrap_or_else(|error| panic!("{error}"));
    let mut effects = ClipEffects::new(arm, &clips, probe, live);
    emit(
        "start",
        json!({"scenario": arm.run_id(), "arm": arm.id, "stall_s": arm.stall_s, "room": effects.live.room_name}),
    );
    emit("notice", json!({"text": DESCRIPTIVE_NOTICE}));

    let result = run_script(&mut permits, &CLIP_SCENARIO, &mut effects).await;
    effects.probe.gate.release();
    let cleanup = cleanup_end_if_needed(&mut permits, &mut effects, &result).await;
    let teardown = effects.live.teardown().await;
    let problems = judge_arm(arm, effects.observation.as_ref(), &teardown, &result);
    if let Some(observation) = &effects.observation {
        emit("arm_result", observation.to_json());
    }
    emit(
        "result",
        json!({
            "scenario": arm.run_id(),
            "script": format!("{result:?}"),
            "cleanup_end": format!("{cleanup:?}"),
            "permits": permits.granted,
            "publisher_error": effects.published_error,
            "participants_left": teardown.participants,
            "room_gone_after_delete": teardown.room_gone_after_delete,
            "readers_unfinished": teardown.readers_unfinished,
            "teardown_notes": teardown.notes,
            "problems": problems,
        }),
    );
    if let Err(error) = cleanup {
        eprintln!("cleanup close failed: {error:?}");
    }
    result.expect("the scripted clip arm completed");
    assert!(problems.is_empty(), "{problems:?}");
}

// Local tests. Everything below runs in the ordinary lane.

fn tone(len: usize, level: i16) -> Vec<i16> {
    (0..len)
        .map(|n| if n % 2 == 0 { level } else { -level })
        .collect()
}

fn wav_bytes(samples: &[i16]) -> Vec<u8> {
    let data: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
    let mut bytes = b"RIFF".to_vec();
    bytes.extend((36 + data.len() as u32).to_le_bytes());
    bytes.extend(b"WAVEfmt ");
    bytes.extend(16u32.to_le_bytes());
    bytes.extend(1u16.to_le_bytes());
    bytes.extend(1u16.to_le_bytes());
    bytes.extend(SAMPLE_RATE.to_le_bytes());
    bytes.extend((SAMPLE_RATE * 2).to_le_bytes());
    bytes.extend(2u16.to_le_bytes());
    bytes.extend(16u16.to_le_bytes());
    bytes.extend(b"data");
    bytes.extend((data.len() as u32).to_le_bytes());
    bytes.extend(data);
    bytes
}

/// A clip directory in the shape of the spoken clips: the manifest, one WAV per
/// clip, parts equal to the answer key's. The audio is a tone, not speech.
fn write_clip_dir(scratch: &Scratch, mutate: impl FnOnce(&mut Value)) -> PathBuf {
    let dir = scratch.path("clips");
    std::fs::create_dir_all(&dir).unwrap();
    let mut entries = Vec::new();
    for (index, id) in CLIP_IDS.iter().enumerate() {
        let bytes = wav_bytes(&tone(1600 * (index + 2), 1000 * (index as i16 + 1)));
        std::fs::write(dir.join(format!("{id}.wav")), &bytes).unwrap();
        entries.push(json!({
            "clip_id": id,
            "file": format!("{id}.wav"),
            "sha256": crate::sha256_hex(&[&bytes]),
            "parts": ANSWER_KEY.iter().filter(|t| t.clip_id == *id).map(|t| t.text).collect::<Vec<_>>(),
        }));
    }
    let mut manifest = json!({
        "contains_speech": true,
        "format": {"container": "wav", "codec": "pcm_s16le", "sample_rate": 16000, "channels": 1},
        "clips": entries,
    });
    mutate(&mut manifest);
    std::fs::write(
        dir.join("manifest.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
    dir
}

/// Declared before running: five arms in a fixed order, one control and the
/// four stalls the leaf names; each arm fits the 60 s setup, 180 s run and 60 s
/// cleanup it is allotted for the longest stall plus the drain, and the
/// manifest declares two planned actions, the close permit as the third, the
/// native-close clip kind and a 300 s cap the controller accepts.
#[test]
fn the_five_arms_declare_the_stalls_and_fit_their_budgets() {
    assert_eq!(
        ARMS.iter()
            .map(|arm| (arm.id, arm.stall_s))
            .collect::<Vec<_>>(),
        [
            ("s0", 0),
            ("s20", 20),
            ("s39", 39),
            ("s41", 41),
            ("s60", 60)
        ]
    );
    assert_eq!(ARMS.iter().filter(|arm| arm.is_control()).count(), 1);
    assert_eq!(CLIP_SETUP_S + CLIP_RUN_S + CLIP_CLEANUP_S, 300);

    // The authored clips are about 37 s; any clip up to the window's slack
    // fits.
    for clip_frames in [3_740u64, 6_000] {
        for arm in &ARMS {
            let schedule = Schedule::new(arm, clip_frames);
            schedule.check_budget().unwrap();
            assert_eq!(
                schedule.total_s(),
                arm.stall_s.max(clip_frames / 100) + DRAIN_S
            );
        }
    }
    assert!(Schedule::new(&ARMS[4], 12_100).check_budget().is_err());
    let scratch = Scratch::new("arm-manifests");
    let candidate = scratch.path("candidate");
    std::fs::write(&candidate, "x").unwrap();
    for arm in &ARMS {
        let manifest = arm.manifest(&candidate, &"c".repeat(64), &"d".repeat(64));
        assert_eq!(manifest["actions"], json!(["candidate_join", "clip_start"]));
        assert_eq!(manifest["action_ceiling"], 3);
        assert_eq!(manifest["run_kind"], "clip");
        assert_eq!(manifest["end_mode"], "native_close");
        assert_eq!(manifest["outer_wall_cap_s"], 300);
        assert_eq!(manifest["cleanup_reserve_s"], 60);
        assert_eq!(manifest["selected_duration_s"], 240);
        let path = scratch.path("manifest.json");
        std::fs::write(&path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        let output = std::process::Command::new("python3")
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/interview-run-budget.py"))
            .args(["validate", "--manifest"])
            .arg(&path)
            .output()
            .expect("python3 runs the controller");
        assert!(
            output.status.success(),
            "{}: {}{}",
            arm.id,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            clip_duration_min(&manifest, None),
            Ok(SCENARIO_DURATION_MIN)
        );
    }
    assert!(arm_for("s40").is_none() && arm_for("a").is_none() && arm_for("").is_none());
    let mut manifest = ARMS[1].manifest(&candidate, &"c".repeat(64), &"d".repeat(64));
    manifest["end_mode"] = json!("manual");
    assert!(clip_duration_min(&manifest, None).is_err());
}

/// Declared before running: the loader accepts a set in the spoken clips'
/// shape and refuses one that declares no speech, has a digest that does not
/// match its bytes, is not 16 kHz mono PCM16, or whose authored parts differ
/// from the answer key; the numbered turns cover the six parts exactly.
#[test]
fn the_clip_loader_checks_speech_digests_format_and_the_answer_keys_text() {
    let scratch = Scratch::new("clips");
    let good = write_clip_dir(&scratch, |_| {});
    let set = ClipSet::load(&good).unwrap();
    assert_eq!(
        set.clips.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(),
        CLIP_IDS
    );
    assert_eq!(set.clips[0].samples.len(), 3200);
    assert_eq!(
        ANSWER_KEY.iter().map(Turn::id).collect::<Vec<_>>(),
        [
            "t1:c01_say_plan/1",
            "t2:c01_say_plan/2",
            "t3:c01_say_plan/3",
            "t4:c01_say_loop/1",
            "t5:c01_say_loop/2",
            "t6:c01_say_loop/3"
        ]
    );
    for (label, mutate) in [
        (
            "speech",
            (|m: &mut Value| m["contains_speech"] = json!(false)) as fn(&mut Value),
        ),
        ("digest", |m| {
            m["clips"][0]["sha256"] = json!("0".repeat(64));
        }),
        ("rate", |m| m["format"]["sample_rate"] = json!(48000)),
        ("text", |m| {
            m["clips"][1]["parts"][2] = json!("something else");
        }),
        ("path", |m| m["clips"][0]["file"] = json!("../x.wav")),
        ("missing", |m| m["clips"] = json!([])),
    ] {
        let scratch = Scratch::new("clips-bad");
        let dir = write_clip_dir(&scratch, mutate);
        assert!(ClipSet::load(&dir).is_err(), "{label} must be refused");
    }
    // A WAV that is stereo, or truncated, is not a clip.
    let mut stereo = wav_bytes(&[1, 2, 3, 4]);
    stereo[22] = 2;
    assert!(parse_wav(&stereo).is_err());
    assert!(parse_wav(&wav_bytes(&[1, 2, 3, 4])[..30]).is_err());
    assert!(parse_wav(b"not a wav").is_err());
}

/// Declared before running: the timeline lays the clips end to end once, pads
/// each to a whole frame, and is silence after the last; a frame past the end
/// is never a repeat of the start.
#[test]
fn the_timeline_plays_each_clip_once_and_never_loops() {
    let set = ClipSet {
        clips: vec![
            Clip {
                id: "one".into(),
                samples: vec![7; 250],
            },
            Clip {
                id: "two".into(),
                samples: vec![9; 160],
            },
        ],
    };
    let timeline = Timeline::new(&set);
    assert_eq!(
        timeline.spans,
        [("one".to_owned(), 0, 2), ("two".to_owned(), 2, 3)]
    );
    assert_eq!(timeline.clip_frames(), 3);
    assert!(timeline.frame(0).iter().all(|s| *s == 7));
    assert_eq!(timeline.frame(1).iter().filter(|s| **s == 7).count(), 90);
    assert!(timeline.frame(2).iter().all(|s| *s == 9));
    for after in [3, 4, 100, 10_000] {
        assert_eq!(
            timeline.frame(after),
            [0i16; FRAME_SAMPLES],
            "frame {after}"
        );
    }
}

/// Records what the publisher captured and when, on the virtual clock.
struct Recorder {
    log: std::sync::Mutex<Vec<(tokio::time::Instant, i16)>>,
}

impl FrameSink for Recorder {
    async fn capture(&self, samples: &[i16]) -> Result<(), String> {
        self.log
            .lock()
            .unwrap()
            .push((tokio::time::Instant::now(), samples[0]));
        Ok(())
    }
}

/// Declared before running, on a paused clock so a 60 s arm takes no wall time:
/// for each arm the publisher sends silence until the reader is ready, holds
/// the gate from the clip's first frame for exactly the stall, releases it at
/// the stall's frame and never earlier, publishes each clip frame exactly once
/// in order at the controller's pace (never ahead of it), and stops after the
/// drain. The control never holds the gate.
#[tokio::test(start_paused = true)]
async fn the_publisher_holds_from_the_first_clip_frame_for_exactly_the_stall() {
    let set = ClipSet {
        clips: vec![
            Clip {
                id: "one".into(),
                samples: vec![5; 16000],
            },
            Clip {
                id: "two".into(),
                samples: vec![6; 16000],
            },
        ],
    };
    let timeline = Timeline::new(&set);
    for arm in &ARMS {
        let schedule = Schedule::new(arm, timeline.clip_frames());
        let gate = DequeueGate::new(Instant::now());
        let sink = Recorder {
            log: Default::default(),
        };
        let ready_after = std::cell::Cell::new(0u32);
        let seen = std::cell::RefCell::new(Vec::new());
        let published = run_publisher(
            &sink,
            &timeline,
            &schedule,
            &gate,
            || {
                ready_after.set(ready_after.get() + 1);
                ready_after.get() > 150
            },
            || {
                seen.borrow_mut()
                    .push((gate.is_held(), tokio::time::Instant::now()));
                None
            },
            |_| {},
        )
        .await;
        assert_eq!(published.error, None, "{}", arm.id);
        assert!(!gate.is_held(), "{}: released at the end", arm.id);
        let log = sink.log.lock().unwrap();
        let start = log[0].0;
        // The clip began at the first frame after 150 ready checks.
        let lead = log.iter().take_while(|(_, marker)| *marker == 0).count();
        assert!(lead >= 150, "{}: silence first, {lead}", arm.id);
        let clip: Vec<i16> = log.iter().map(|(_, m)| *m).skip(lead).take(200).collect();
        assert_eq!(&clip[..100], &[5; 100][..], "{}: clip one once", arm.id);
        assert_eq!(&clip[100..], &[6; 100][..], "{}: clip two once", arm.id);
        assert_eq!(
            log.iter().filter(|(_, m)| *m != 0).count(),
            200,
            "{}: the clips are published once and never again",
            arm.id
        );
        for (position, (at, _)) in log.iter().enumerate() {
            assert!(
                *at >= start + Duration::from_millis(FRAME_MS * position as u64),
                "{}: frame {position} was captured early",
                arm.id
            );
        }
        assert_eq!(
            log.len() as u64,
            lead as u64 + schedule.total_frames,
            "{}",
            arm.id
        );

        // The gate was held for the stall's frames after the lead and not for
        // one more: the abort probe runs once per frame, before the gate moves.
        let held: Vec<bool> = seen.borrow().iter().map(|(held, _)| *held).collect();
        let held_frames = held.iter().filter(|held| **held).count() as u64;
        assert_eq!(held_frames, arm.stall_frames(), "{}: held frames", arm.id);
        if arm.is_control() {
            assert!(held.iter().all(|held| !*held));
        }
        let stats = gate.stats();
        assert_eq!(stats.held_at.is_some(), !arm.is_control(), "{}", arm.id);
        assert_eq!(stats.released_at.is_some(), !arm.is_control(), "{}", arm.id);
    }
}

/// Declared before running: a publisher whose reader never becomes ready
/// gives up with an error, and a sink failure or an abort stops it, with the
/// gate released in every case.
#[tokio::test(start_paused = true)]
async fn the_publisher_gives_up_and_always_releases_the_gate() {
    let set = ClipSet {
        clips: vec![Clip {
            id: "one".into(),
            samples: vec![5; 1600],
        }],
    };
    let timeline = Timeline::new(&set);
    let schedule = Schedule::new(&ARMS[1], timeline.clip_frames());

    let gate = DequeueGate::new(Instant::now());
    let sink = Recorder {
        log: Default::default(),
    };
    let never = run_publisher(
        &sink,
        &timeline,
        &schedule,
        &gate,
        || false,
        || None,
        |_| {},
    )
    .await;
    assert!(never.error.as_deref().unwrap().contains("never read"));
    assert!(never.began.is_none() && !gate.is_held());

    let gate = DequeueGate::new(Instant::now());
    let count = std::cell::Cell::new(0u32);
    let aborted = run_publisher(
        &sink,
        &timeline,
        &schedule,
        &gate,
        || true,
        || {
            count.set(count.get() + 1);
            (count.get() > 400).then(|| "stop".to_owned())
        },
        |_| {},
    )
    .await;
    assert_eq!(aborted.error.as_deref(), Some("stop"));
    assert!(
        !gate.is_held(),
        "an abort in the middle of the hold releases"
    );
}

// The answer key, the comparison and the report.

fn observation(arm: &Arm, lines: &[(u64, &str)]) -> Observation {
    Observation {
        arm: arm.id.to_owned(),
        stall_s: arm.stall_s,
        clip_ids: CLIP_IDS.iter().map(|id| (*id).to_owned()).collect(),
        turn_ids: ANSWER_KEY.iter().map(Turn::id).collect(),
        frames_planned: 100,
        frames_published: 100,
        complete: true,
        clips: vec![
            ClipPublication {
                clip_id: "c01_say_plan".into(),
                first_ms: 0,
                last_ms: 20_790,
            },
            ClipPublication {
                clip_id: "c01_say_loop".into(),
                first_ms: 20_800,
                last_ms: 37_400,
            },
        ],
        held_ms: (!arm.is_control()).then_some(0),
        released_ms: (!arm.is_control()).then_some(arm.stall_s * 1000),
        held_arrivals: u64::from(!arm.is_control()),
        dequeued_frames: 10,
        first_dequeue_ms: Some(1),
        last_dequeue_ms: Some(2),
        first_after_release_ms: None,
        frames_after_release: 0,
        per_second: vec![(0, 100), (-1, 3)],
        transcripts: lines
            .iter()
            .map(|(at, text)| (*at, (*text).to_owned()))
            .collect(),
        dropped_bytes: 0,
        quarantined_bytes: 0,
        sdk_overflow_logger: true,
        sdk_overflow_logs: Vec::new(),
        sdk_overflow_unstored: 0,
    }
}

const FULL: [(u64, &str); 6] = [
    (2_000, "Stage one. We are now in the algorithm stage. "),
    (
        6_000,
        "My constraints are at most 10,000 numbers, exactly one valid pair. ",
    ),
    (14_000, "Codename saffron heron. "),
    (24_000, "For each value I check the complement "),
    (28_000, "before adding the current index to the map. "),
    (
        34_000,
        "Should I return the two positions in ascending order? ",
    ),
];

fn statuses(report: &ArmReport) -> Vec<Vec<Status>> {
    report
        .turns
        .iter()
        .map(|turn| turn.statuses.clone())
        .collect()
}

/// Declared before running, from the authored text: a transcript carrying all
/// six turns is intact in every arm; an arm lacking turn 1 whose control has it
/// has a missing finding; a repeated codename is a repeat; a turn heard after
/// the budget is delayed; and normalization copes with punctuation, case and
/// "10,000".
#[test]
fn the_answer_key_counts_missing_repeated_and_delayed_turns() {
    let control = observation(&ARMS[0], &FULL);
    let mut lines = FULL.to_vec();
    lines.remove(0);
    lines.insert(2, (15_000, "saffron heron again. "));
    lines.push((90_000, "ignored text. "));
    let late = [(70_000, "For each value I check the complement ")];
    let mut stalled = observation(&ARMS[1], &lines);
    stalled
        .transcripts
        .retain(|(_, text)| !text.contains("For each value"));
    stalled
        .transcripts
        .extend(late.iter().map(|(at, t)| (*at, (*t).to_owned())));
    let reports = compare(&[control, stalled]);
    assert_eq!(
        reports.len(),
        5,
        "a report row exists for every declared arm"
    );
    assert!(
        reports[0]
            .turns
            .iter()
            .all(|t| t.statuses == [Status::Intact])
    );
    let s20 = statuses(&reports[1]);
    assert_eq!(s20[0], [Status::Finding(Kind::Missing)]);
    assert_eq!(s20[1], [Status::Intact]);
    assert_eq!(s20[2], [Status::Finding(Kind::Repeated)]);
    assert_eq!(s20[3], [Status::Finding(Kind::Delayed)], "heard at 70 s");
    assert_eq!(reports[1].findings(Kind::Missing), 1);
    assert_eq!(reports[1].findings(Kind::Repeated), 1);
    assert_eq!(reports[1].findings(Kind::Delayed), 1);
    assert_eq!(reports[1].inconclusive(), 0);
    for not_run in &reports[2..] {
        assert!(not_run.observation.is_none() && not_run.turns.is_empty());
    }
    assert_eq!(normalize("It's 10,000-- A, b!"), "it s 10 000 a b");
}

/// Declared before running: a miss that the unstalled control also shows is
/// inconclusive and never counted as a loss, so is a repeat or a delay the
/// control shares; the control's own findings are baselines; with no control
/// every finding is inconclusive. Fragments split mid-word are still heard
/// when the pieces join.
#[test]
fn a_miss_also_present_in_the_control_is_inconclusive_not_a_loss() {
    let mut without_two: Vec<(u64, &str)> = FULL.to_vec();
    without_two.remove(1);
    let control = observation(&ARMS[0], &without_two);
    let stalled = observation(&ARMS[3], &without_two);
    let mut lost_three = without_two.clone();
    lost_three.remove(1);
    let lost = observation(&ARMS[4], &lost_three);
    let reports = compare(&[control, stalled.clone(), lost]);
    assert_eq!(statuses(&reports[0])[1], [Status::Baseline(Kind::Missing)]);
    assert_eq!(
        statuses(&reports[3])[1],
        [Status::Inconclusive(
            Kind::Missing,
            "also present in the control"
        )]
    );
    assert_eq!(reports[3].findings(Kind::Missing), 0);
    assert_eq!(reports[3].inconclusive(), 1);
    assert_eq!(statuses(&reports[4])[2], [Status::Finding(Kind::Missing)]);
    assert_eq!(
        statuses(&reports[4])[1],
        [Status::Inconclusive(
            Kind::Missing,
            "also present in the control"
        )]
    );
    assert_eq!(reports[4].findings(Kind::Missing), 1);

    let no_control = compare(&[stalled]);
    assert_eq!(
        statuses(&no_control[3])[1],
        [Status::Inconclusive(
            Kind::Missing,
            "no control observation"
        )]
    );
    assert_eq!(no_control[3].findings(Kind::Missing), 0);

    // Late in the control too: the stall is not to blame.
    let mut slow_control = observation(&ARMS[0], &FULL);
    slow_control.transcripts[0].0 = 60_000;
    let mut slow = observation(&ARMS[1], &FULL);
    slow.transcripts[0].0 = 70_000;
    let reports = compare(&[slow_control, slow]);
    assert_eq!(statuses(&reports[0])[0], [Status::Baseline(Kind::Delayed)]);
    assert_eq!(
        statuses(&reports[1])[0],
        [Status::Inconclusive(
            Kind::Delayed,
            "also present in the control"
        )]
    );

    let split = observation(
        &ARMS[0],
        &[
            (1_000, "the algo"),
            (2_000, "rithm. we a"),
            (3_000, "re now"),
            (4_000, " saff"),
            (5_000, "ron he"),
            (6_000, "ron"),
        ],
    );
    let raw = raw_findings(&split);
    assert!(!raw[0].missing && !raw[2].missing);
    assert_eq!(raw[0].completed_ms, Some(3_000));
    assert_eq!(raw[2].first_ms, Some(5_000));
    assert_eq!(raw[2].completed_ms, Some(6_000));
}

/// Declared before running: the arm_result line survives the round trip through
/// the log's own JSON, noise lines are skipped, and the report names every
/// declared arm, says plainly that one clip per arm is descriptive and that
/// exact SDK drops are not inferred from the transcript, and marks an arm with
/// no result as not run.
#[test]
fn the_report_reads_the_log_and_states_its_limits_for_every_arm() {
    let control = observation(&ARMS[0], &FULL);
    let mut lost = observation(&ARMS[4], &FULL[3..]);
    lost.dropped_bytes = 320;
    lost.quarantined_bytes = 4;
    let mut log = String::from("running 1 test\nLIVE_OBS not json\n");
    for item in [&control, &lost] {
        log.push_str(&format!(
            "LIVE_OBS {}\n",
            json!({"kind": "arm_result", "fields": item.to_json()})
        ));
    }
    log.push_str("LIVE_OBS {\"kind\":\"transcript_line\",\"fields\":{}}\nother\n");
    let parsed = parse_arm_results(&log);
    assert_eq!(parsed, [control.clone(), lost]);

    let report = render_report(&parsed);
    assert!(report.contains("One clip per arm is descriptive evidence only"));
    assert!(report.contains("exact SDK drops are not inferred from the transcript"));
    for arm in &ARMS {
        assert!(report.contains(&format!("ARM {} ", arm.id)), "{}", arm.id);
    }
    assert_eq!(report.matches("NOT RUN").count(), 3);
    assert!(report.contains("dropped_bytes=320 quarantined_bytes=4"));
    assert!(report.contains("t1:c01_say_plan/1: missing"));
    assert!(report.contains("totals: missing 3"));
    assert!(report.contains("Should I return the two positions"));
    let twice = render_report(&[control.clone(), control]);
    assert!(twice.contains("2 results for s0"));
}

// The dequeue gate.

/// A stream a test feeds by hand.
struct Fed(tokio::sync::mpsc::UnboundedReceiver<u32>);

impl futures_util::Stream for Fed {
    type Item = u32;
    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        context: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<u32>> {
        self.0.poll_recv(context)
    }
}

/// Declared before running: with the gate held and frames waiting, the consumer
/// takes nothing for as long as it is held, while another task and a timer on
/// the same runtime keep running; on release it takes the frames in order, the
/// first transmission is stamped after the release, and the held arrival is
/// counted once. A consumer that was cancelled while held has taken nothing.
#[tokio::test]
async fn a_held_dequeue_takes_nothing_while_the_runtime_keeps_running() {
    let (feed, frames) = tokio::sync::mpsc::unbounded_channel();
    for n in 1..=5 {
        feed.send(n).unwrap();
    }
    let gate = Arc::new(DequeueGate::new(Instant::now()));
    gate.hold();
    let ticks = Arc::new(AtomicUsize::new(0));
    let ticker = tokio::spawn({
        let ticks = std::sync::Arc::clone(&ticks);
        async move {
            loop {
                tokio::time::sleep(Duration::from_millis(5)).await;
                ticks.fetch_add(1, Ordering::SeqCst);
            }
        }
    });
    let mut stream = Fed(frames);

    // A consumer cancelled while held takes nothing.
    {
        let cancelled = probe::gated_next(Some(gate.as_ref()), &mut stream);
        assert!(
            tokio::time::timeout(Duration::from_millis(50), cancelled)
                .await
                .is_err()
        );
    }
    assert_eq!(gate.stats().frames, 0);

    let consumer = tokio::spawn({
        let gate = std::sync::Arc::clone(&gate);
        async move {
            let mut taken = Vec::new();
            while taken.len() < 5 {
                taken.push(
                    probe::gated_next(Some(gate.as_ref()), &mut stream)
                        .await
                        .unwrap(),
                );
            }
            taken
        }
    });
    let ticks_before = ticks.load(Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(
        ticks.load(Ordering::SeqCst) >= ticks_before + 10,
        "the rest of the runtime ran while the consumer was held"
    );
    assert!(!consumer.is_finished());
    assert_eq!(gate.stats().frames, 0, "nothing was taken while held");
    assert_eq!(
        gate.stats().held_arrivals,
        2,
        "one per waiting consumer, not per wake"
    );
    assert!(gate.is_held());

    gate.release();
    let released = Instant::now();
    assert_eq!(consumer.await.unwrap(), [1, 2, 3, 4, 5]);
    ticker.abort();
    let stats = gate.stats();
    assert_eq!(stats.frames, 5);
    assert_eq!(stats.frames_after_release, 5);
    assert!(
        stats.first_after_release_at.unwrap() >= stats.released_at.unwrap(),
        "a frame is stamped after the release it waited for"
    );
    assert!(stats.released_at.unwrap() <= released);
    assert!(stats.held_at.unwrap() <= stats.released_at.unwrap());
    drop(feed);
}

/// Declared before running: with no gate, or an open one, the dequeue takes at
/// once and records the frames by second; hold and release are idempotent and
/// the per-second tally rebases to a new epoch.
#[tokio::test]
async fn an_open_gate_takes_at_once_and_tallies_by_second_from_its_epoch() {
    let (feed, frames) = tokio::sync::mpsc::unbounded_channel();
    let mut stream = Fed(frames);
    feed.send(1).unwrap();
    assert_eq!(probe::gated_next(None, &mut stream).await, Some(1));
    let gate = DequeueGate::new(Instant::now() - Duration::from_secs(3));
    feed.send(2).unwrap();
    assert_eq!(probe::gated_next(Some(&gate), &mut stream).await, Some(2));
    assert_eq!(gate.stats().held_arrivals, 0);
    assert_eq!(
        gate.stats().per_second.keys().copied().collect::<Vec<_>>(),
        [3]
    );
    gate.rebase(Instant::now() + Duration::from_secs(2));
    feed.send(3).unwrap();
    assert_eq!(probe::gated_next(Some(&gate), &mut stream).await, Some(3));
    assert_eq!(
        gate.stats().per_second.keys().copied().collect::<Vec<_>>(),
        [-2]
    );
    assert_eq!(gate.stats().first_after_release_at, None, "never released");
    gate.release();
    assert_eq!(
        gate.stats().released_at,
        None,
        "release of an open gate is nothing"
    );
    gate.hold();
    let held = gate.stats().held_at;
    gate.hold();
    assert_eq!(gate.stats().held_at, held);
    drop(feed);
    assert_eq!(probe::gated_next(None, &mut stream).await, None);
}

// The recording peer: the real SDK stream, the real pump and a fake Gemini.

/// What the fake Gemini socket received: when, and the audio's first sample.
struct WireAudio {
    at: Instant,
    marker: i16,
    bytes: usize,
}

fn recording_peer(
    listener: tokio::net::TcpListener,
    wire: Arc<std::sync::Mutex<Vec<WireAudio>>>,
) -> tokio::task::JoinHandle<()> {
    use base64::{Engine, engine::general_purpose::STANDARD};
    tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
        socket.next().await.unwrap().unwrap();
        socket
            .send(Message::Text(r#"{"setupComplete":{}}"#.into()))
            .await
            .unwrap();
        while let Some(Ok(frame)) = socket.next().await {
            if frame.is_close() {
                break;
            }
            let Ok(text) = frame.to_text() else { continue };
            let value: Value = serde_json::from_str(text).unwrap();
            if let Some(data) = value["realtimeInput"]["audio"]["data"].as_str() {
                let bytes = STANDARD.decode(data).unwrap();
                wire.lock().unwrap().push(WireAudio {
                    at: Instant::now(),
                    marker: i16::from_le_bytes([bytes[0], bytes[1]]),
                    bytes: bytes.len(),
                });
            }
        }
    })
}

use super::super::{Rig, isolated, listener, numbered_frame};
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;

/// Declared before running, per row of the arm table in frames (captured in
/// one burst: the SDK's rule does not depend on pacing). The consumer is held
/// while the loop's other arms run; during the hold the fake Gemini receives no
/// audio, the room control and timer arms still run and the gate counted the
/// consumer arriving. After the release the wire carries one 320-byte message
/// per frame, in order, with the numbered samples intact: all of them when the
/// stall fits the 4000-frame store, and only the newest 4000 when it does not
/// (so frame N-3999 first), with the oldest dropped silently. The first
/// transmission is stamped after the release, and the control is never held.
#[tokio::test]
async fn the_recording_peer_sees_nothing_while_held_and_the_retained_frames_after() {
    if isolated(
        "live::continuity::the_recording_peer_sees_nothing_while_held_and_the_retained_frames_after",
    ) {
        return;
    }
    for (label, frames, hold) in [
        ("control", 2000i16, false),
        ("s20", 2000, true),
        ("s39", 3900, true),
        ("s41", 4100, true),
        ("s60", 6000, true),
    ] {
        assert!(overflow_log::install(), "{label}: another logger was first");
        overflow_log::CAPTURE.clear();
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

        let source = NativeAudioSource::new(AudioSourceOptions::default(), SAMPLE_RATE, 1, 0);
        let track = LocalAudioTrack::create_audio_track(
            "continuity-wire",
            RtcAudioSource::Native(source.clone()),
        );
        rig.media.audio = Some(crate::livekit::media::candidate_audio_stream(
            track.rtc_track(),
        ));
        if hold {
            probe.gate.hold();
        }
        let captured_from = Instant::now();
        for marker in 1..=frames {
            source
                .capture(&numbered_frame(marker, FRAME_SAMPLES).data)
                .await
                .unwrap();
        }
        let retained = frames.min(4000);
        let first_marker = frames - retained + 1;

        let mut controls = 0usize;
        let mut ticks = 0usize;
        let mut released_at = None;
        let mut ticker = tokio::time::interval(Duration::from_millis(10));
        let ended = tokio::time::Instant::now() + Duration::from_secs(20);
        let mut quiet_since: Option<tokio::time::Instant> = None;
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
                    ticks += 1;

                    // Room controls and keepalive polling stand in as the arms
                    // that must keep running: a control is answered each tick.
                    controls += 1;
                    if hold && released_at.is_none() && ticks == 30 {
                        assert!(wire.lock().unwrap().is_empty(), "{label}: audio reached Gemini while held");
                        assert_eq!(probe.gate.stats().frames, 0, "{label}");
                        assert!(probe.gate.stats().held_arrivals >= 1, "{label}");
                        released_at = Some(Instant::now());
                        probe.gate.release();
                    }
                    if probe.gate.stats().frames == u64::from(retained.unsigned_abs()) {
                        let since = *quiet_since.get_or_insert_with(tokio::time::Instant::now);
                        if since.elapsed() > Duration::from_millis(300)
                            && wire.lock().unwrap().len() == usize::try_from(retained).unwrap()
                        {
                            break;
                        }
                    }
                    assert!(tokio::time::Instant::now() < ended, "{label}: never drained");
                }
            }
        }
        assert!(controls >= 30, "{label}: other arms kept running");
        rig.gemini.shutdown().await.unwrap();
        server.await.unwrap();

        let wire = wire.lock().unwrap();
        assert_eq!(
            wire.len(),
            usize::try_from(retained).unwrap(),
            "{label}: one message per retained frame"
        );
        assert!(wire.iter().all(|item| item.bytes == 320), "{label}");
        let markers: Vec<i16> = wire.iter().map(|item| item.marker).collect();
        assert_eq!(
            markers,
            (first_marker..=frames).collect::<Vec<i16>>(),
            "{label}: the newest frames, in order, none repeated"
        );
        let stats = probe.gate.stats();
        assert_eq!(stats.frames, u64::from(retained.unsigned_abs()), "{label}");

        // Declared: the SDK logs the queue's first drop and every 100th, so N =
        // 1, 100, 200, ... up to the drops; none when nothing dropped.
        let dropped = u64::from((frames - retained).unsigned_abs());
        let mut want: Vec<u64> = Vec::new();
        if dropped > 0 {
            want.push(1);
            want.extend((1..=dropped / 100).map(|step| step * 100));
        }
        let snapshot = overflow_log::snapshot();
        let seen: Vec<OverflowLog> = snapshot
            .records
            .iter()
            .map(|record| OverflowLog::at(record, captured_from))
            .collect();
        assert_eq!(
            seen.iter()
                .filter_map(|line| line.dropped)
                .collect::<Vec<_>>(),
            want,
            "{label}: the SDK's overflow lines"
        );
        assert_eq!(snapshot.unstored, 0, "{label}");
        for line in &seen {
            assert!(line.target.starts_with("libwebrtc"), "{label}: {line:?}");
            assert!(line.message.contains("native audio stream queue overflow"));
        }
        if hold {
            // The drops happen while the capture burst runs, before the hold is
            // released at tick 30.
            let released_ms = u64::try_from(
                released_at
                    .unwrap()
                    .saturating_duration_since(captured_from)
                    .as_millis(),
            )
            .unwrap();
            assert!(
                seen.iter().all(|line| line.at_ms <= released_ms as i64),
                "{label}: {seen:?} vs release at {released_ms} ms"
            );
        }
        if hold {
            let released_at = released_at.unwrap();
            assert!(
                wire[0].at >= released_at,
                "{label}: first transmission precedes the release"
            );
            assert!(stats.first_after_release_at.unwrap() >= released_at);
            assert_eq!(stats.frames_after_release, stats.frames, "{label}");
            assert!(stats.held_at.is_some());
        } else {
            assert_eq!(stats.held_arrivals, 0, "{label}: the control is never held");
            assert!(stats.held_at.is_none() && stats.released_at.is_none());
            assert!(wire[0].at >= captured_from);
        }
    }
}
