// Part of recovery.rs: included in place, so every item here shares that file's
// module.

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
    let session = crate::livekit::tests::fixture_session_at(&endpoint, &keys, &boot, None)
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
        || plant_open(std::rc::Rc::clone(&plant)),
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
        // On the quota grid a 90 s recovery lands at 120.3 s.
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
