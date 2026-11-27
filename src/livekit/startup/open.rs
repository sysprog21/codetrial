//! Opening the interview: the first Gemini session, retried while it will not
//! connect, and the owned state the room loop runs on once the candidate has
//! joined and both sides are talking.

// This module reads the hub's imports and types through `use
// crate::livekit::*`.
#[cfg_attr(
    not(test),
    expect(
        clippy::wildcard_imports,
        reason = "the module reads its hub through crate::livekit::*"
    )
)]
use crate::livekit::*;
use readiness::{
    StartupContext, account_startup, close_and_account, probe_live_output, publish_startup_failure,
    publish_startup_readiness, ready_runtime_activity, startup_failure_line,
    startup_failure_receipt, startup_output_limit, startup_ready_notice, startup_state,
};

/// Opens a session that remembers nothing, retrying while the budget allows.
///
/// `Err` means credentials are unavailable or retries stopped, ending the
/// interview. Split from `replace_gemini_session` because a loop with its own
/// exit inside an arm of a match inside an arm of a match put the give-up path
/// four levels deep in a function that is otherwise a sequence of steps.
async fn open_cold_session(
    interview: InterviewContext<'_>,
    restarts: &mut usize,
) -> Result<GeminiLiveSession, Box<dyn std::error::Error + Send + Sync>> {
    let mut ready = None;
    let deadline =
        tokio::time::Instant::from_std(interview.started_at) + crate::gemini::FIRST_OPEN_LIMIT;
    open_cold_session_into(interview, restarts, None, Some(deadline), &mut ready).await?;
    Ok(ready.take().expect("successful open owns its session"))
}

pub(in crate::livekit) async fn open_cold_session_into(
    interview: InterviewContext<'_>,
    restarts: &mut usize,
    room: Option<&RoomLink>,
    deadline: Option<tokio::time::Instant>,
    ready: &mut Option<GeminiLiveSession>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Boxed: the retry loop is the largest state any caller's wait holds, and a
    // debug build polls it on the room task's stack.
    Box::pin(retry_cold_open_into(
        interview.keys,
        restarts,
        deadline,
        ready,
        || async {
            let opened = interview.open_live(None).await;
            #[cfg(test)]
            if let (Some(room), Ok(session)) = (room, &opened) {
                room.observe_live_reader(session);
            }
            opened
        },
        async |notice| {
            let Some(room) = room else { return };
            match tokio::time::timeout(
                RECONNECT_NOTICE_LIMIT,
                publish_interviewer_state(room, &notice),
            )
            .await
            {
                Ok(Ok(())) => {}
                Ok(Err(error)) => eprintln!("publishing reconnect wait failed ({error})"),
                Err(_) => eprintln!("publishing reconnect wait timed out; retrying without it"),
            }
        },
        |until| {
            let sleep = tokio::time::sleep_until(until);
            #[cfg(test)]
            let sleep = room_link::observe_retry_backoff(room, until, sleep);
            sleep
        },
    ))
    .await
}

/// Test convenience wrapper for the caller-owned cold-open retry loop.
#[cfg(test)]
pub(in crate::livekit) async fn retry_cold_open<F, Fut, N, NFut>(
    keys: &GeminiKeys,
    restarts: &mut usize,
    deadline: Option<tokio::time::Instant>,
    open: F,
    notify: N,
) -> Result<GeminiLiveSession, Box<dyn std::error::Error + Send + Sync>>
where
    N: FnMut(serde_json::Value) -> NFut,
    NFut: Future<Output = ()>,
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<GeminiLiveSession, Box<dyn std::error::Error + Send + Sync>>>,
{
    let mut ready = None;
    retry_cold_open_into(
        keys,
        restarts,
        deadline,
        &mut ready,
        open,
        notify,
        tokio::time::sleep_until,
    )
    .await?;
    Ok(ready.take().expect("successful open owns its session"))
}

/// `deadline` is a first open's budget: a cooldown that outlasts it fails at
/// once, with its own cause, instead of waiting for the outer timeout.
pub(in crate::livekit) async fn retry_cold_open_into<F, Fut, N, NFut, B, BFut>(
    keys: &GeminiKeys,
    restarts: &mut usize,
    deadline: Option<tokio::time::Instant>,
    ready: &mut Option<GeminiLiveSession>,
    mut open: F,
    mut notify: N,
    mut backoff: B,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>>
where
    N: FnMut(serde_json::Value) -> NFut,
    NFut: Future<Output = ()>,
    B: FnMut(tokio::time::Instant) -> BFut,
    BFut: Future<Output = ()>,
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<GeminiLiveSession, Box<dyn std::error::Error + Send + Sync>>>,
{
    // Announced alongside the attempt it describes rather than ahead of it, so
    // a slow room does not delay the retry; still finished before the loop
    // returns, so it cannot land after the notice that clears the banner.
    let mut attempt_notice = None;

    // The last failure a retry moved past. When the rotation then runs dry, it
    // is what the keys were refused with, and the dry rotation says nothing.
    let mut moved_past = None;
    loop {
        let announce = attempt_notice.take().map(&mut notify);
        let opening = async {
            match open().await {
                Ok(session) => {
                    // The caller owns a successful setup before another await,
                    // even while its concurrent room notice remains pending.
                    *ready = Some(session);
                    Ok(())
                }
                Err(error) => Err(error),
            }
        };
        let (opened, ()) = tokio::join!(opening, async {
            if let Some(announce) = announce {
                announce.await;
            }
        });
        let (reason, until) = match opened {
            Ok(()) => return Ok(()),

            // A known cooldown that cannot fit the first-open budget cannot
            // recover here. Preserve its cause rather than letting the outer
            // timeout replace it with an unexplained transport failure.
            Err(error)
                if crate::gemini::exhausted_until(error.as_ref()).is_some_and(|until| {
                    deadline
                        .is_some_and(|deadline| tokio::time::Instant::from_std(until) >= deadline)
                }) =>
            {
                return Err(error);
            }

            // Every key is out on quota. A sole key would ride out the same
            // rate limit under this budget, so a rotation waits too.
            Err(error)
                if crate::gemini::exhausted_until(error.as_ref()).is_some()
                    && take_restart_attempt(restarts, Duration::ZERO) =>
            {
                let until = crate::gemini::exhausted_until(error.as_ref())
                    .unwrap_or_else(std::time::Instant::now);
                eprintln!("Gemini keys are all cooling down ({error}); waiting for the first back");
                (RECONNECT_QUOTA, until.into())
            }

            // A sole key's 429 lands here rather than above: one key out on
            // quota is not yet an exhausted rotation.
            Err(error)
                if crate::gemini::retry_live_open(error.as_ref(), keys.has_backups())
                    && take_restart_attempt(restarts, Duration::ZERO) =>
            {
                eprintln!("Gemini could not be reached ({error}); retrying cold session");
                let reason = if crate::gemini::is_quota_failure(error.as_ref()) {
                    RECONNECT_QUOTA
                } else {
                    RECONNECT_RETRYING
                };
                moved_past = Some(error);
                (reason, tokio::time::Instant::now() + COLD_OPEN_BACKOFF)
            }
            Err(error) => {
                eprintln!("Gemini could not be reached ({error}); stopping cold session attempts");
                return Err(match moved_past {
                    Some(rejected) if crate::gemini::rotation_ran_dry(error.as_ref()) => rejected,
                    _ => error,
                });
            }
        };

        // Due from before the notice, so a slow publish is spent inside the
        // wait the page was told about rather than added to it. A key already
        // back has no wait worth announcing.
        let wait = until.saturating_duration_since(tokio::time::Instant::now());
        if !wait.is_zero() {
            notify(reconnect_wait(reason, Some(wait))).await;
        }
        backoff(until).await;

        // The estimate has run out, but the attempt it led to can take a
        // connect and a setup timeout, so the page drops the number rather than
        // show it for half a minute.
        attempt_notice = Some(reconnect_wait(reason, None));
    }
}

/// Everything the interview loop needs, owned, once the candidate has joined
/// and both sides are talking.
///
/// Owned and not a set of borrows: the loop builds `InterviewContext` and
/// `RoomIdentities` out of these fields, and a struct that held those views
/// alongside the values they point at would be borrowing itself. `'a` is the
/// caller's, carried only by `boot`.
pub(in crate::livekit) struct OpenSession<'a> {
    pub(in crate::livekit) room: RoomLink,
    pub(in crate::livekit) events: tokio::sync::Mutex<RoomEventReader>,
    pub(in crate::livekit) agent_identity: String,
    pub(in crate::livekit) candidate_identity: String,
    pub(in crate::livekit) boot: RuntimeBootstrap<'a>,
    pub(in crate::livekit) output_audio: OutputAudio,
    pub(in crate::livekit) gemini: crate::gemini::GeminiLiveSession,
    pub(in crate::livekit) media: CandidateMedia,
    pub(in crate::livekit) turn: TurnState,
    pub(in crate::livekit) started_at: Instant,
    pub(in crate::livekit) restarts: usize,
}

pub(in crate::livekit) enum OpenSessionOutcome<'a> {
    Started(Box<OpenSession<'a>>),
    NoCandidate,
    FailedBeforeCandidate(Box<dyn std::error::Error + Send + Sync>),
}

/// Joins, waits for a candidate, brings up audio and Gemini, and greets.
///
/// Split from [`run_room`] because none of it repeats: it runs once, in order,
/// and every line of it is a step that either succeeds or ends the interview
/// before it starts. What is left in `run_room` is the part that runs many
/// times, which is the part worth reading as a loop.
///
/// `NoCandidate` is nobody joined. That is not an error: the room was named,
/// the candidate never arrived, and there is nothing to run or report.
pub(in crate::livekit) async fn open_session<'a>(
    config: &'a AgentConfig,
    keys: &Arc<GeminiKeys>,
    room_name: &'a str,
    now_seconds: u64,
    session_id: u64,
    slot: Option<&dyn RoomLease>,
    log_startup_failure: impl FnOnce(String),
) -> Result<OpenSessionOutcome<'a>, Box<dyn std::error::Error + Send + Sync>> {
    // A page can give its start up before this task gets to the room, and
    // joining a room nobody will come to spends a connection for nothing.
    if slot.is_some_and(|slot| slot.is_released()) {
        eprintln!("start released before the agent joined room={room_name}");
        return Ok(OpenSessionOutcome::NoCandidate);
    }
    let agent_identity = agent_identity(room_name);
    let joined = async {
        let (room, mut events) = join_room(
            config,
            room_name,
            &agent_identity,
            now_seconds,
            MAX_BOARD_BYTES,
        )
        .await?;
        let room = RoomLink::live(room);
        let mut agent_state = crate::livekit::AgentState::default();
        let opened_attributes = crate::livekit::agent_state::publish_opening_attributes(
            &room,
            &mut agent_state,
            config.gemini_silence_ms,
        )
        .await;
        room.close_on_error(opened_attributes).await?;

        // The candidate's metadata picks the problem, so nothing else can start
        // until someone joins.
        let mut deferred_events = Vec::new();
        let arrival =
            candidate_metadata(room.connected(), &mut events, &mut deferred_events, slot).await;
        let candidate = room.close_on_error(arrival).await?;
        Ok::<_, Box<dyn std::error::Error + Send + Sync>>((
            room,
            events,
            agent_state,
            candidate,
            deferred_events,
        ))
    }
    .await;
    let (room, events, mut agent_state, candidate, mut deferred_events) = match joined {
        Ok(joined) => joined,
        Err(error) => {
            log_startup_failure(startup_failure_line(
                &config.gemini_live_model,
                room_name,
                session_id,
                error.as_ref(),
            ));
            return Ok(OpenSessionOutcome::FailedBeforeCandidate(error));
        }
    };

    // Claimed here, under the dispatch lock, so a release racing this arrival
    // either ends the start before it is claimed or finds it already joined.
    let candidate = candidate.filter(|_| slot.is_none_or(|slot| slot.claim_join()));
    let Some((candidate_identity, candidate_metadata)) = candidate else {
        eprintln!("no candidate joined room={room_name}; leaving it");

        // Dropping the room does not disconnect it, and the slot drops right
        // after this, so a fixed room's next start would staff it twice.
        room.leave().await;
        return Ok(OpenSessionOutcome::NoCandidate);
    };
    let events = tokio::sync::Mutex::new(RoomEventReader::from(events));
    let boot = candidate_bootstrap(config, room_name, Some(&candidate_metadata));
    eprintln!(
        "starting interview: room={} problem={} duration={}min",
        boot.room_name, boot.problem.id, boot.duration_min
    );
    let mut room_closed = false;
    let started = account_startup(
        config,
        room_name,
        session_id,
        log_startup_failure,
        async |startup_usage| {
            start_interview(
                Startup {
                    config,
                    keys,
                    room_name,
                    session_id,
                    room: &room,
                    events: &events,
                    agent_identity: &agent_identity,
                    candidate_identity: &candidate_identity,
                    boot: &boot,
                },
                &mut agent_state,
                &mut deferred_events,
                &mut room_closed,
                startup_usage,
            )
            .await
        },
    )
    .await;
    let (output_audio, gemini, media, turn, started_at, restarts) = match started {
        Ok(started) => started,
        Err(error) => {
            if !room_closed
                && let Ok(packet) =
                    browser_packet(TOPIC_CONTROL, &startup_state(Some(error.as_ref())))
            {
                let id = crate::sha256_hex(&[&packet.payload]);
                publish_startup_failure(
                    room.publish_data(packet),
                    startup_failure_receipt(&events, &candidate_identity, &id),
                )
                .await;
            }
            return room.close_on_error(Err(error)).await;
        }
    };

    Ok(OpenSessionOutcome::Started(Box::new(OpenSession {
        room,
        events,
        agent_identity,
        candidate_identity,
        boot,
        output_audio,
        gemini,
        media,
        turn,
        started_at,
        restarts,
    })))
}

/// What the start of an interview reads and never changes, once the candidate
/// has joined.
#[derive(Clone, Copy)]
struct Startup<'s, 'a> {
    config: &'a AgentConfig,
    keys: &'s Arc<GeminiKeys>,
    room_name: &'a str,
    session_id: u64,
    room: &'s RoomLink,
    events: &'s tokio::sync::Mutex<RoomEventReader>,
    agent_identity: &'s str,
    candidate_identity: &'s str,
    boot: &'s RuntimeBootstrap<'a>,
}

type Started = (
    OutputAudio,
    GeminiLiveSession,
    CandidateMedia,
    TurnState,
    Instant,
    usize,
);

/// Brings up audio and Gemini, then waits for the opening to produce output
/// and tells the candidate the interview is ready. The interview clock starts
/// at that first output, not at setup: a socket that opened but never answers
/// must not spend the candidate's time.
async fn start_interview(
    startup: Startup<'_, '_>,
    agent_state: &mut crate::livekit::AgentState,
    deferred_events: &mut Vec<TimedRoomEvent>,
    room_closed: &mut bool,
    startup_usage: &mut crate::gemini::TokenUsage,
) -> Result<Started, Box<dyn std::error::Error + Send + Sync>> {
    let Startup {
        config,
        keys,
        room_name,
        session_id,
        room,
        events,
        agent_identity,
        candidate_identity,
        boot,
    } = startup;

    // Generated beside the setup below and picked up whenever it lands: a tool
    // round before it is ready simply plays nothing.
    let tool_ack = Arc::new(std::sync::OnceLock::new());
    let mut tool_ack_task = None;
    if config.gemini_tool_ack {
        let (keys, slot) = (Arc::clone(keys), Arc::clone(&tool_ack));
        let (voice, accent) = (boot.voice.to_string(), boot.accent.map(str::to_string));
        let task = tokio::spawn(async move {
            match crate::gemini::tool_ack_clip(
                &keys,
                &voice,
                accent.as_deref(),
                GEMINI_OUTPUT_AUDIO_SAMPLE_RATE,
            )
            .await
            {
                Ok(pcm) => {
                    let _ = slot.set(media::trim_silence(&pcm, GEMINI_OUTPUT_AUDIO_SAMPLE_RATE));
                }
                Err(error) => eprintln!("tool acknowledgement not generated: {error}"),
            }
        });
        tool_ack_task = Some(media::AbortOnDrop(task.abort_handle()));
    }

    // Only the Gemini session overlaps, and only because it touches nothing in
    // the room. Isolation and publication cannot: isolation exists so that one
    // agent is in the room when the candidate starts subscribing, and running
    // it beside the publish put our audio track into a room that still held the
    // duplicate. Whichever the browser picked was a coin toss, and picking the
    // one about to be removed meant the candidate heard nothing at all.
    //
    // "Neither needs the other's return value" was the wrong test. The
    // dependency is on room state, not on data.
    let setup_began = Instant::now();
    let mut restarts = 0;
    let mut room_ready = Duration::ZERO;
    let mut gemini_ready = Duration::ZERO;
    let (mut output_audio, mut gemini) = tokio::try_join!(
        async {
            isolate_local_agent(config, room_name, agent_identity).await?;
            let published =
                publish_output_audio(room.connected(), GEMINI_OUTPUT_AUDIO_SAMPLE_RATE).await;
            room_ready = setup_began.elapsed();
            published
        },
        async {
            let interview = InterviewContext {
                config,
                keys,
                boot,
                #[cfg(test)]
                live_endpoint: None,
                started_at: setup_began,
                candidate_identity,
                events,
                slot: None,
            };
            let opened = crate::gemini::first_open_within(
                crate::gemini::FIRST_OPEN_LIMIT,
                open_cold_session(interview, &mut restarts),
            )
            .await;
            gemini_ready = setup_began.elapsed();
            opened
        },
    )?;

    output_audio.adopt_tool_ack(tool_ack, tool_ack_task);

    // Which branch held the greeting back: an earlier Gemini connect can only
    // help when Gemini is the later of the two.
    eprintln!(
        "timing: room and Gemini session ready {:.2}s after the candidate joined (room {:.2}s, gemini {:.2}s)",
        setup_began.elapsed().as_secs_f64(),
        room_ready.as_secs_f64(),
        gemini_ready.as_secs_f64()
    );

    // Only prompt construction uses this provisional clock. The opening starts
    // with its full budget, and no watch or round transition runs before ready.
    let provisional = Instant::now();
    let mut turn = TurnState {
        state: initial_runtime_state(boot, provisional),
        agent_state: std::mem::take(agent_state),
        activity: runtime_activity(config, provisional, session_id),
        probes: TurnProbes::default(),
        streams: Default::default(),
        turns: SpeakerTurns::default(),
        transcripts: Default::default(),
    };
    turn.state
        .evidence_ledger
        .record_model_input(ModelInputKind::LiveSetup, &boot.instructions);
    let mut media = CandidateMedia::new();

    // Before the greeting, not after: these arrived while we were waiting for
    // the candidate, and one of them is what attaches the microphone. Greeting
    // first would open the interview deaf for as long as Gemini takes to
    // answer.
    *deferred_events = attach_waiting_media(
        &mut media,
        &mut turn.streams,
        &mut gemini,
        candidate_identity,
        boot.candidate_video,
        std::mem::take(deferred_events),
    )
    .await?;

    let started_at = open_interview(
        startup,
        &mut gemini,
        &mut turn,
        &mut media,
        deferred_events,
        room_closed,
        startup_usage,
    )
    .await?;
    Ok((output_audio, gemini, media, turn, started_at, restarts))
}

/// Asks for the opening and starts the interview at its first output: the
/// probe, the clock it sets, and the ready notice that tells the page so.
async fn open_interview(
    startup: Startup<'_, '_>,
    gemini: &mut GeminiLiveSession,
    turn: &mut TurnState,
    media: &mut CandidateMedia,
    deferred_events: &mut Vec<TimedRoomEvent>,
    room_closed: &mut bool,
    startup_usage: &mut crate::gemini::TokenUsage,
) -> Result<Instant, Box<dyn std::error::Error + Send + Sync>> {
    let Startup {
        config,
        keys,
        room_name,
        session_id,
        room,
        events,
        candidate_identity,
        boot,
        ..
    } = startup;
    let greeting = with_timer(&turn.state, boot.greeting.clone());
    let opening_requested_at = Instant::now();
    let probed = probe_live_output(
        gemini,
        &mut turn.state,
        &greeting,
        startup_output_limit(config),
        &mut StartupContext {
            media,
            streams: &mut turn.streams,
            candidate_identity,
            candidate_video: boot.candidate_video,
            deferred_events,
            room_closed,
        },
        &mut &*events,
    )
    .await;
    let started_at = match probed {
        Ok(started_at) => started_at,
        Err(error) => {
            record_probe_failure(gemini, keys, *room_closed);
            close_and_account(gemini, startup_usage).await;
            return Err(error);
        }
    };

    // The candidate's clock, not a third one: the browser starts counting when
    // it receives ready, and the elapsed time it carries places this instant on
    // the browser's clock. Every consumer below reads this as "when the
    // interview started", and that is when it started.
    turn.state.started_at = started_at;
    (turn.activity, turn.probes) = ready_runtime_activity(
        config,
        room_name,
        started_at,
        opening_requested_at,
        session_id,
        &greeting,
    );
    eprintln!("{}", turn.probes.latency.anchor(started_at));
    let ready = startup_ready_notice(started_at, turn.activity.startup_ready_at);
    publish_startup_readiness(
        gemini,
        startup_usage,
        events,
        deferred_events,
        room_closed,
        publish_interviewer_state(room, &ready),
    )
    .await?;

    // Replayed by the room loop ahead of anything newer, and taken over with
    // the reader by the report handler after End, so a packet received while
    // ready was publishing is answered in order either way.
    events.lock().await.replay(std::mem::take(deferred_events));
    Ok(started_at)
}

/// Attaches the media that arrived while the candidate was awaited, and hands
/// back what is not media, in receive order, for the room loop.
pub(in crate::livekit) async fn attach_waiting_media(
    media: &mut CandidateMedia,
    streams: &mut CandidateStreams,
    gemini: &mut GeminiLiveSession,
    candidate_identity: &str,
    candidate_video: bool,
    events: Vec<TimedRoomEvent>,
) -> Result<Vec<TimedRoomEvent>, Box<dyn std::error::Error + Send + Sync>> {
    let mut deferred = Vec::new();
    for (received_at, event) in events {
        if !handle_media_event(
            media,
            streams,
            gemini,
            candidate_identity,
            candidate_video,
            &event,
        )
        .await?
        {
            deferred.push((received_at, event));
        }
    }
    Ok(deferred)
}

/// A probe that failed with its room still there failed on the provider, so
/// the key it used is marked. One the room's closing ended says nothing about
/// the key.
pub(in crate::livekit) fn record_probe_failure(
    gemini: &GeminiLiveSession,
    keys: &GeminiKeys,
    room_closed: bool,
) {
    if !room_closed {
        gemini.record_failure(keys);
    }
}
