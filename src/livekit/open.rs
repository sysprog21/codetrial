//! Opening the interview: the first Gemini session, retried while it will not
//! connect, and the owned state the room loop runs on once the candidate has
//! joined and both sides are talking.

// This module reads the hub's imports and types through `use super::*`.
#[cfg_attr(
    not(test),
    expect(
        clippy::wildcard_imports,
        reason = "the module reads its hub through super::*"
    )
)]
use super::*;

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
    open_cold_session_into(interview, restarts, None, &mut ready).await?;
    Ok(ready.take().expect("successful open owns its session"))
}

pub(super) async fn open_cold_session_into(
    interview: InterviewContext<'_>,
    restarts: &mut usize,
    room: Option<&RoomLink>,
    ready: &mut Option<GeminiLiveSession>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    retry_cold_open_into(
        interview.keys,
        restarts,
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
    )
    .await
}

/// Test convenience wrapper for the caller-owned cold-open retry loop.
#[cfg(test)]
pub(super) async fn retry_cold_open<F, Fut, N, NFut>(
    keys: &GeminiKeys,
    restarts: &mut usize,
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
        &mut ready,
        open,
        notify,
        tokio::time::sleep_until,
    )
    .await?;
    Ok(ready.take().expect("successful open owns its session"))
}

pub(super) async fn retry_cold_open_into<F, Fut, N, NFut, B, BFut>(
    keys: &GeminiKeys,
    restarts: &mut usize,
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
                (reason, tokio::time::Instant::now() + COLD_OPEN_BACKOFF)
            }
            Err(error) => {
                eprintln!("Gemini could not be reached ({error}); stopping cold session attempts");
                return Err(error);
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
pub(super) struct OpenSession<'a> {
    pub(super) room: RoomLink,
    pub(super) events: tokio::sync::Mutex<tokio::sync::mpsc::UnboundedReceiver<RoomEvent>>,
    pub(super) agent_identity: String,
    pub(super) candidate_identity: String,
    pub(super) boot: RuntimeBootstrap<'a>,
    pub(super) output_audio: OutputAudio,
    pub(super) gemini: crate::gemini::GeminiLiveSession,
    pub(super) media: CandidateMedia,
    pub(super) turn: TurnState,
    pub(super) started_at: Instant,
    pub(super) restarts: usize,
}

pub(super) enum OpenSessionOutcome<'a> {
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
pub(super) async fn open_session<'a>(
    config: &'a AgentConfig,
    keys: &Arc<GeminiKeys>,
    room_name: &'a str,
    now_seconds: u64,
    session_id: u64,
    slot: Option<&crate::dispatch::Slot>,
) -> Result<OpenSessionOutcome<'a>, Box<dyn std::error::Error + Send + Sync>> {
    // A page can give its start up before this task gets to the room, and
    // joining a room nobody will come to spends a connection for nothing.
    if slot.is_some_and(crate::dispatch::Slot::is_released) {
        eprintln!("start released before the agent joined room={room_name}");
        return Ok(OpenSessionOutcome::NoCandidate);
    }
    let agent_identity = agent_identity(room_name);
    let (room, mut events) = match join_room(config, room_name, &agent_identity, now_seconds).await
    {
        Ok(joined) => joined,
        Err(error) => return Ok(OpenSessionOutcome::FailedBeforeCandidate(error)),
    };
    let room = RoomLink::live(room);
    let mut agent_state = session::AgentState::default();
    let opened_attributes =
        session::publish_opening_attributes(&room, &mut agent_state, config.gemini_silence_ms)
            .await;
    if let Err(error) = room.close_on_error(opened_attributes).await {
        return Ok(OpenSessionOutcome::FailedBeforeCandidate(error));
    }

    // The candidate's metadata picks the problem, so nothing else can start
    // until someone joins.
    let mut deferred_events = Vec::new();
    let arrival =
        candidate_metadata(room.connected(), &mut events, &mut deferred_events, slot).await;
    let candidate = match room.close_on_error(arrival).await {
        Ok(candidate) => candidate,
        Err(error) => return Ok(OpenSessionOutcome::FailedBeforeCandidate(error)),
    };

    // Claimed here, under the dispatch lock, so a release racing this arrival
    // either ends the start before it is claimed or finds it already joined.
    let candidate = candidate.filter(|_| slot.is_none_or(crate::dispatch::Slot::claim_join));
    let Some((candidate_identity, candidate_metadata)) = candidate else {
        eprintln!("no candidate joined room={room_name}; leaving it");

        // Dropping the room does not disconnect it, and the slot drops right
        // after this, so a fixed room's next start would staff it twice.
        leave_room(&room).await;
        return Ok(OpenSessionOutcome::NoCandidate);
    };
    let events = tokio::sync::Mutex::new(events);
    let boot = candidate_bootstrap(config, room_name, Some(&candidate_metadata));
    eprintln!(
        "starting interview: room={} problem={} duration={}min",
        boot.room_name, boot.problem.id, boot.duration_min
    );

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
    let joined = tokio::try_join!(
        async {
            isolate_local_agent(config, room_name, &agent_identity, now_seconds).await?;
            let published =
                publish_output_audio(room.connected(), GEMINI_OUTPUT_AUDIO_SAMPLE_RATE).await;
            room_ready = setup_began.elapsed();
            published
        },
        async {
            let interview = InterviewContext {
                config,
                keys,
                boot: &boot,
                #[cfg(test)]
                live_endpoint: None,
                started_at: setup_began,
                candidate_identity: &candidate_identity,
                events: &events,
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
    );
    let (mut output_audio, mut gemini) = room.close_on_error(joined).await?;

    output_audio.tool_ack = tool_ack;
    output_audio.tool_ack_task = tool_ack_task;

    // Which branch held the greeting back: an earlier Gemini connect can only
    // help when Gemini is the later of the two.
    eprintln!(
        "timing: room and Gemini session ready {:.2}s after the candidate joined (room {:.2}s, gemini {:.2}s)",
        setup_began.elapsed().as_secs_f64(),
        room_ready.as_secs_f64(),
        gemini_ready.as_secs_f64()
    );

    // The candidate's clock, not a third one. The browser starts counting when
    // its own connect resolves, which is this instant seen from the other side,
    // and stamping again here put the agent a second or two behind the tab for
    // the rest of the interview. Every consumer below reads this as "when the
    // interview started", and that is when it started.
    let started_at = setup_began;

    let mut turn = TurnState {
        state: initial_runtime_state(&boot, started_at),
        agent_state: std::mem::take(&mut agent_state),
        activity: runtime_activity(config, started_at, session_id),
        turns: SpeakerTurns::default(),
        transcripts: Default::default(),
    };
    turn.activity.latency.bind(
        room.name(),
        session_id,
        turn.activity.live_socket,
        config.gemini_silence_ms,
    );
    eprintln!("{}", turn.activity.latency.anchor(started_at));
    turn.state
        .evidence_ledger
        .record_model_input(ModelInputKind::LiveSetup, &boot.instructions);
    let mut media = CandidateMedia::new();

    // Before the greeting, not after: these arrived while we were waiting for
    // the candidate, and one of them is what attaches the microphone. Greeting
    // first would open the interview deaf for as long as Gemini takes to
    // answer.
    for event in deferred_events.drain(..) {
        handle_media_event(
            &mut media,
            &mut gemini,
            &candidate_identity,
            boot.candidate_video,
            &event,
        )
        .await?;
    }

    let greeting = with_timer(&turn.state, boot.greeting.clone());
    let greeted = send_model_text(
        &mut gemini,
        &mut turn.state,
        ModelInputKind::Turn,
        TurnCause::Turn,
        &greeting,
    )
    .await;
    room.close_on_error(greeted).await?;
    turn.activity
        .mark_prompted(Instant::now(), Some(&greeting), false);

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
