//! Startup readiness: the interview clock starts only once the Live model has
//! answered its opening, not once its setup succeeded. This probes for that
//! first output without consuming it, publishes ready (or why startup failed)
//! to the candidate, keeps the room events that arrive meanwhile in receive
//! order, and replays ready to a candidate who returns or asks again.

// This module reads the hub's imports and types through `use super::*`.
#[cfg_attr(
    not(test),
    expect(
        clippy::wildcard_imports,
        reason = "the module reads its hub through super::*"
    )
)]
use super::*;

/// Readiness follows an already paid greeting and is required to start the
/// browser clock. Unlike a best-effort reconnect notice, it can wait out a slow
/// publish. First open (30s), maximum greeting wait (120s), this publish (15s),
/// shutdown (2s) and failure notice (1s) leave headroom in the browser's 180s
/// wait, which the page counts from the same room join.
pub(super) const STARTUP_READY_PUBLICATION_LIMIT: Duration = Duration::from_secs(15);
/// Publication must not turn a busy room into another unbounded event buffer.
pub(super) const STARTUP_READY_EVENT_LIMIT: usize = 256;

pub(super) enum CandidatePresenceChange {
    Left(Instant),
    Returned(Instant),
}

impl CandidatePresence {
    pub(super) fn projected(&self, changes: impl Iterator<Item = CandidatePresenceChange>) -> Self {
        let mut presence = self.clone();
        for change in changes {
            match change {
                CandidatePresenceChange::Left(at) => presence.left(at),
                CandidatePresenceChange::Returned(at) if !presence.gave_up(at) => {
                    presence.returned();
                }
                CandidatePresenceChange::Returned(_) => {}
            }
        }
        presence
    }

    /// Judges pending presence without waiting for unrelated queued work. A
    /// timely return may already be queued, while a long packet handler must
    /// not postpone an absence that has actually expired.
    pub(super) fn with_queued(&self, events: &RoomEventReader, candidate: &str) -> Self {
        self.projected(
            events
                .deferred
                .iter()
                .filter_map(|(at, event)| match event {
                    RoomEvent::ParticipantDisconnected(participant)
                        if participant.identity().0 == candidate =>
                    {
                        Some(CandidatePresenceChange::Left(*at))
                    }
                    RoomEvent::ParticipantConnected(participant)
                        if participant.identity().0 == candidate =>
                    {
                        Some(CandidatePresenceChange::Returned(*at))
                    }
                    _ => None,
                }),
        )
    }
}

/// Receive time survives startup and ready publication; replay is not a new
/// departure or return, and cannot restart the candidate's presence grace.
pub(super) type TimedRoomEvent = (Instant, RoomEvent);

pub(super) struct RoomEventReader<Event = RoomEvent> {
    pub(super) events: tokio::sync::mpsc::UnboundedReceiver<Event>,
    pub(super) deferred: std::collections::VecDeque<(Instant, Event)>,
    /// The page asked for the ready notice while a socket replacement held
    /// the room and could not answer; the replacement's caller does.
    pub(super) startup_sync_pending: bool,
}

impl<Event> From<tokio::sync::mpsc::UnboundedReceiver<Event>> for RoomEventReader<Event> {
    fn from(events: tokio::sync::mpsc::UnboundedReceiver<Event>) -> Self {
        Self {
            events,
            deferred: Default::default(),
            startup_sync_pending: false,
        }
    }
}

impl<Event> RoomEventReader<Event> {
    async fn recv_timed(&mut self) -> Option<(Instant, Event)> {
        match self.deferred.pop_front() {
            Some(event) => Some(event),
            None => self
                .events
                .recv()
                .await
                .map(|event| (tokio::time::Instant::now().into_std(), event)),
        }
    }

    pub(super) async fn recv(&mut self) -> Option<Event> {
        self.recv_timed().await.map(|(_, event)| event)
    }

    pub(super) fn try_recv(&mut self) -> Result<Event, tokio::sync::mpsc::error::TryRecvError> {
        match self.deferred.pop_front() {
            Some((_, event)) => Ok(event),
            None => self.events.try_recv(),
        }
    }
}

pub(super) async fn next_timed_room_event(
    events: &tokio::sync::Mutex<RoomEventReader>,
) -> Option<TimedRoomEvent> {
    events.lock().await.recv_timed().await
}

/// Latency includes the opening wait. The probe has already observed output,
/// even if publishing ready delays its delivery to the room loop: the watchdog
/// must judge generation progress rather than ask for the first output again.
pub(super) fn ready_runtime_activity(
    config: &AgentConfig,
    room_name: &str,
    started_at: Instant,
    opening_requested_at: Instant,
    session_id: u64,
    greeting: &str,
) -> RuntimeActivity {
    let mut activity = runtime_activity(config, started_at, session_id);
    activity.latency.bind(
        room_name,
        session_id,
        activity.live_socket,
        config.gemini_silence_ms,
    );
    activity.mark_prompted(opening_requested_at, Some(greeting), false);
    activity.resume_reply_wait(started_at);
    activity.buffered_startup_output_at = Some(started_at);
    activity.startup_ready_at = Some(
        crate::current_epoch_millis()
            .saturating_sub(crate::agent::saturating_ms(started_at.elapsed())),
    );
    activity
}

pub(super) fn startup_state(
    error: Option<&(dyn std::error::Error + 'static)>,
) -> serde_json::Value {
    let mut notice = interviewer_state(false);
    notice["startup"] = if error.is_some() { "failed" } else { "ready" }.into();
    if let Some(error) = error {
        notice["reason"] = if error.is::<StartupDeliveryError>() {
            "delivery"
        } else {
            crate::gemini::startup_failure_reason(error)
        }
        .into();
    }
    notice
}

pub(super) fn startup_ready_notice(
    started_at: Instant,
    wall_clock: Option<u64>,
) -> serde_json::Value {
    let mut notice = startup_state(None);
    notice["startedAt"] = wall_clock.into();

    // Only elapsed time crosses clock domains. The wall-clock stamp identifies
    // the original opening but cannot be subtracted from a browser's clock.
    notice["elapsedMs"] = crate::agent::saturating_ms(started_at.elapsed()).into();
    notice
}

/// The normal opening generation is the probe, not a second paid turn. Setup
/// alone proves nothing, and no watchdog or replacement runs before it answers.
pub(super) async fn probe_live_output(
    gemini: &mut GeminiLiveSession,
    state: &mut RuntimeState,
    greeting: &str,
    limit: Duration,
    startup: &mut StartupContext<'_>,
    input: &mut impl StartupInputs,
) -> Result<Instant, Box<dyn std::error::Error + Send + Sync>> {
    let deadline = tokio::time::Instant::now() + limit;
    let result = tokio::time::timeout_at(deadline, async {
        send_model_text(
            gemini,
            state,
            ModelInputKind::Turn,
            TurnCause::Turn,
            greeting,
        )
        .await?;

        // These survive cancellation of the output wait when media wins the
        // select. Recreating either would restart the deadline or ping on every
        // microphone frame. The generation above is never part of that select.
        let mut keepalive = tokio::time::interval(crate::gemini::KEEPALIVE_INTERVAL);
        keepalive.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                ready = gemini.wait_for_model_output(&mut keepalive) => {
                    return ready;
                }
                input = input.next(startup.media) => {
                    let received_at = Instant::now();
                    if matches!(&input, StartupInput::Room(None | Some(RoomEvent::Disconnected { .. }))) {
                        *startup.room_closed = true;
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::Interrupted,
                            "room closed before startup output",
                        ).into());
                    }
                    let forwarded = forward_startup_input(gemini, startup, input, received_at).await;
                    if let Err(error) = forwarded {
                        // A full queue while looking must not replace the
                        // write's own failure as the reason.
                        if let Ok(Some(received_at)) = gemini.buffered_startup_output(deadline.into_std()) {
                            return Ok(received_at);
                        }
                        return Err(error);
                    }
                }
            }
        }
    })
    .await;
    let result = match result {
        Ok(result) if result.is_err() || tokio::time::Instant::now() < deadline => result,

        // The outer deadline can cancel a media write before its own timeout
        // runs. The independent reader may already have queued the opening.
        // Tokio also polls the inner future first, even at an elapsed deadline.
        _ => gemini
            .buffered_startup_output(deadline.into_std())
            .and_then(|received_at| {
                if received_at.is_some() && gemini.audio_write_queued() {
                    // The cancelled flush already handed these samples to the
                    // sink. The next write must not enqueue a second copy.
                    startup.media.audio_bytes.clear();
                }
                received_at.ok_or_else(|| {
                    std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "Gemini did not produce startup output before the deadline",
                    )
                    .into()
                })
            }),
    };
    result.map_err(|error| {
        if *startup.room_closed {
            error
        } else {
            gemini.startup_error(error)
        }
    })
}

async fn forward_startup_input(
    gemini: &mut GeminiLiveSession,
    startup: &mut StartupContext<'_>,
    input: StartupInput,
    received_at: Instant,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    match input {
        StartupInput::Room(event) => {
            if let Some(event) = event
                && !handle_media_event(
                    startup.media,
                    gemini,
                    startup.candidate_identity,
                    startup.candidate_video,
                    &event,
                )
                .await?
            {
                startup.deferred_events.push((received_at, event));
            }
        }
        StartupInput::Audio(frame) => {
            let ended = frame.is_none();
            if frame.as_ref().is_some_and(media::frame_has_voice) {
                startup.media.voiced_at = Some(received_at);
            }
            let sent = pump_audio(startup.media, gemini, frame).await;
            release_if_ended(&mut startup.media.audio, ended);
            sent?;
        }
        StartupInput::Video(frame) => {
            let speaking = startup.media.voiced_recently(received_at);
            pump_video(startup.media, gemini, frame, speaking).await?;
        }
        // The room loop applies the original presence history after readiness.
        StartupInput::ParticipantLeft(event) => startup.deferred_events.push((received_at, event)),
    }
    Ok(())
}

pub(super) struct StartupContext<'a> {
    pub(super) media: &'a mut CandidateMedia,
    pub(super) candidate_identity: &'a str,
    pub(super) candidate_video: bool,
    pub(super) deferred_events: &'a mut Vec<TimedRoomEvent>,
    pub(super) room_closed: &'a mut bool,
}

pub(super) enum StartupInput {
    Room(Option<RoomEvent>),
    ParticipantLeft(RoomEvent),
    Audio(Option<::livekit::webrtc::audio_frame::AudioFrame<'static>>),
    Video(Option<::livekit::webrtc::video_frame::BoxVideoFrame>),
}

pub(super) trait StartupInputs: Send {
    fn next(&mut self, media: &mut CandidateMedia) -> impl Future<Output = StartupInput> + Send;
}

impl StartupInputs for &tokio::sync::Mutex<RoomEventReader> {
    async fn next(&mut self, media: &mut CandidateMedia) -> StartupInput {
        let events = *self;
        tokio::select! {
            event = async { events.lock().await.recv().await } => match event {
                Some(event @ RoomEvent::ParticipantDisconnected(_)) => StartupInput::ParticipantLeft(event),
                event => StartupInput::Room(event),
            },
            frame = next_audio_frame(&mut media.audio), if media.audio.is_some() => StartupInput::Audio(frame),
            frame = next_video_frame(&mut media.video), if media.video.is_some() => StartupInput::Video(frame),
        }
    }
}

pub(super) fn startup_output_limit(config: &AgentConfig) -> Duration {
    Duration::from_secs(u64::from(
        config
            .gemini_first_output_s
            .max(crate::config::DEFAULT_GEMINI_FIRST_OUTPUT_S)
            .min(config.gemini_reply_timeout_s),
    ))
}

#[derive(Debug)]
pub(super) struct StartupDeliveryError(pub(super) Box<dyn std::error::Error + Send + Sync>);

impl std::fmt::Display for StartupDeliveryError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "publishing startup readiness failed: {}", self.0)
    }
}

impl std::error::Error for StartupDeliveryError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.0.as_ref())
    }
}

/// A failed ready notice can follow a paid generation. Stop the reader before
/// draining so the common startup summary receives each observation once.
pub(super) async fn publish_startup_readiness(
    gemini: &mut GeminiLiveSession,
    usage: &mut crate::gemini::TokenUsage,
    events: &tokio::sync::Mutex<RoomEventReader>,
    deferred: &mut Vec<TimedRoomEvent>,
    room_closed: &mut bool,
    publication: impl Future<Output = Result<(), Box<dyn std::error::Error + Send + Sync>>>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Keep observation separate from replay: a timely return must survive a
    // slow publish, and the first runtime watch may run before its event arm.
    // No handlers, media writes or reconnect notices run inside this budget.
    // The bound is on what this publish collects: events the probe already
    // deferred were received over the opening and say nothing about it.
    let collected_before = deferred.len();
    let collecting = async {
        tokio::pin!(publication);
        loop {
            tokio::select! {
                biased;
                result = &mut publication => return result,
                event = next_timed_room_event(events) => {
                    let Some(event) = event else {
                        *room_closed = true;
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::Interrupted,
                            "room closed during startup readiness publication",
                        ).into());
                    };
                    if let RoomEvent::Disconnected { reason } = &event.1 {
                        *room_closed = true;
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::Interrupted,
                            format!("room disconnected during startup readiness publication: {reason:?}"),
                        ).into());
                    }
                    if deferred.len() - collected_before >= STARTUP_READY_EVENT_LIMIT {
                        return Err(std::io::Error::other(
                            "startup readiness room event limit reached",
                        ).into());
                    }
                    deferred.push(event);
                }
            }
        }
    };
    let result = tokio::time::timeout(STARTUP_READY_PUBLICATION_LIMIT, collecting)
        .await
        .unwrap_or_else(|_| {
            Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "publishing startup readiness timed out",
            )
            .into())
        });
    if result.is_err() {
        close_and_account(gemini, usage).await;
    }
    result.map_err(|error| StartupDeliveryError(error).into())
}

/// A failed start can still have been billed. Only observed counters are
/// reported; absence of observations is not a claim of zero cost.
pub(super) async fn close_and_account(
    gemini: &mut GeminiLiveSession,
    usage: &mut crate::gemini::TokenUsage,
) {
    if let Err(error) = gemini.shutdown().await {
        eprintln!("Gemini close failed during startup ({error}); recording usage anyway");
    }
    for observed in gemini.drain_usage() {
        usage.add(observed);
    }
}

/// Publication only queues the notice. Keep its sender in the roster until
/// the candidate acknowledges it, within the same budget and without
/// retransmission.
pub(super) async fn publish_startup_failure(
    publication: impl Future<Output = Result<(), Box<dyn std::error::Error + Send + Sync>>>,
    receipt: impl Future<Output = ()>,
) {
    let delivered = tokio::time::timeout(RECONNECT_NOTICE_LIMIT, async {
        if publication.await.is_ok() {
            receipt.await;
        }
    })
    .await;
    if delivered.is_err() {
        eprintln!("startup failure notice unacknowledged within its limit; leaving anyway");
    }
}

/// Waits for the candidate to acknowledge failure notice `id`, or for the room
/// or the candidate to go, whichever comes first.
pub(super) async fn startup_failure_receipt(
    events: &tokio::sync::Mutex<RoomEventReader>,
    candidate: &str,
    id: &str,
) {
    let mut events = events.lock().await;
    while let Some(event) = events.recv().await {
        match event {
            RoomEvent::DataReceived {
                payload,
                topic,
                participant,
                ..
            } => {
                let sender = participant
                    .as_ref()
                    .map(|participant| participant.identity().0);
                if is_startup_receipt(topic.as_deref(), sender.as_deref(), candidate, &payload, id)
                {
                    return;
                }
            }
            RoomEvent::Disconnected { .. } => return,
            RoomEvent::ParticipantDisconnected(participant)
                if participant.identity().0 == candidate =>
            {
                return;
            }
            _ => {}
        }
    }
}

pub(super) fn is_startup_receipt(
    topic: Option<&str>,
    sender: Option<&str>,
    candidate: &str,
    payload: &[u8],
    id: &str,
) -> bool {
    // The topic first: an editor packet waiting in the same queue is not parsed
    // just to be thrown away.
    topic == Some(TOPIC_CONTROL)
        && interview_packet(topic, sender, candidate, payload).is_some_and(|(_, value)| {
            value["type"] == "startup_received" && value["deliveryId"] == id
        })
}

pub(super) fn startup_failure_line(
    model: &str,
    room_name: &str,
    session_id: u64,
    error: &(dyn std::error::Error + Send + Sync + 'static),
) -> String {
    // No generation was asked for, so there is no usage.
    startup_usage_line(
        model,
        room_name,
        session_id,
        error,
        &crate::gemini::TokenUsage::default(),
    )
}

/// A failed start's summary with whatever usage it observed. A socket may have
/// opened before the failure, so no count is claimed.
fn startup_usage_line(
    model: &str,
    room_name: &str,
    session_id: u64,
    error: &(dyn std::error::Error + Send + Sync + 'static),
    usage: &crate::gemini::TokenUsage,
) -> String {
    let outcome = LiveOutcome::from_error(error, LiveOutcome::Error);
    live_usage_line(room_name, session_id, model, 0, outcome, None, usage)
}

pub(super) async fn account_startup<T>(
    config: &AgentConfig,
    room_name: &str,
    session_id: u64,
    log_startup_failure: impl FnOnce(String),
    opening: impl AsyncFnOnce(
        &mut crate::gemini::TokenUsage,
    ) -> Result<T, Box<dyn std::error::Error + Send + Sync>>,
) -> Result<T, Box<dyn std::error::Error + Send + Sync>> {
    let mut startup_usage = crate::gemini::TokenUsage::default();
    let opened = opening(&mut startup_usage).await;

    // Room setup can fail before a Gemini socket exists. Both that path and a
    // paid opening failure must reach exactly one summary with observed usage.
    if let Err(error) = &opened {
        log_startup_failure(startup_usage_line(
            &config.gemini_live_model,
            room_name,
            session_id,
            error.as_ref(),
            &startup_usage,
        ));
    }
    opened
}

pub(super) async fn handle_candidate_packet(
    room: &RoomLink,
    context: &mut GeminiEventContext<'_>,
    presence: &CandidatePresence,
    interview: InterviewContext<'_>,
    topic: &str,
    payload: &serde_json::Value,
) -> Result<ControlFlow<()>, Box<dyn std::error::Error + Send + Sync>> {
    if topic == TOPIC_CONTROL && payload["type"] == "startup_sync" {
        resend_startup_readiness(room, context, presence, interview.candidate_identity).await;
        return Ok(ControlFlow::Continue(()));
    }
    handle_data_packet(room, context, interview, topic, payload, true).await
}

pub(super) async fn candidate_returned(
    room: &RoomLink,
    context: &mut GeminiEventContext<'_>,
    presence: &mut CandidatePresence,
    candidate: &str,
    received_at: Instant,
) {
    if presence.gave_up(received_at) || context.state.ended {
        return;
    }
    presence.returned();
    context.state.announce_thinking();
    // A rejoined page starts with an empty checklist.
    context.state.framework_published.clear();
    resend_startup_readiness(room, context, presence, candidate).await;
}

/// Reliable data is not a mailbox for an absent candidate. A return replays
/// the successful probe, not its generation. The browser also queries on SDK
/// resume, which need not produce a server-side `ParticipantConnected` event,
/// and after its data channel is usable if an early return notice was missed
/// again.
pub(super) async fn resend_startup_readiness(
    room: &RoomLink,
    context: &GeminiEventContext<'_>,
    presence: &CandidatePresence,
    candidate: &str,
) {
    if context.state.ended || presence.gave_up(Instant::now()) {
        return;
    }
    let Some(started_at) = context.activity.startup_ready_at else {
        return;
    };
    let notice = startup_ready_notice(context.state.started_at, Some(started_at));
    let sent = match browser_packet(TOPIC_CONTROL, &notice) {
        Ok(mut packet) => {
            packet.destination_identities = vec![candidate.into()];
            room.publish_data_within_notice_limit(packet).await.is_ok()
        }
        Err(_) => false,
    };
    if !sent {
        eprintln!("startup state resync failed; room={}", room.name());
    }
}
