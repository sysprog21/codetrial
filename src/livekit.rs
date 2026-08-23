//! The agent's side of a room: join, stream the candidate's audio and video to
//! Gemini, publish Gemini's audio back, and produce the report when the
//! interview ends.
//!
//! This is the largest module in the tree, so here is its shape. The regions
//! below are cohesive and appear in this order; a split, when one is wanted,
//! should follow these lines rather than a line count.
//!
//! - `run_room` and `join_room`: the lifecycle, and the only entry point.
//! - Room admin over the LiveKit REST API (`isolate_local_agent` through
//!   `evict_duplicate_agent`): finding and removing a duplicate agent left
//!   behind by a previous run.
//! - Event handling (`handle_media_event`, `handle_data_packet`,
//!   `handle_gemini_event`): the three sources that drive the session.
//! - `OutputAudio` and its worker: Gemini's audio, paced onto a LiveKit track.
//! - Media codecs (`append_pcm16_bytes` through `encode_video_frame_jpeg`):
//!   pure functions, no room state, each covered by the tests at the bottom of
//!   this file.
//! - Report building (`publish_report` through `report_data_packet`).

use std::collections::HashMap;
use std::ops::ControlFlow;
use std::time::{Duration, Instant};

use ::livekit::DisconnectReason;
use ::livekit::ParticipantKind;
use ::livekit::data_stream::api::StreamTextOptions;
use ::livekit::options::TrackPublishOptions;
use ::livekit::prelude::{
    DataPacket, LocalAudioTrack, LocalTrack, RemoteAudioTrack, RemoteParticipant, RemoteTrack,
    Room, RoomEvent, RoomOptions, RtcAudioSource, TrackSource,
};
use ::livekit::webrtc::audio_frame::AudioFrame;
use ::livekit::webrtc::audio_source::AudioSourceOptions;
use ::livekit::webrtc::audio_source::native::NativeAudioSource;
use ::livekit::webrtc::audio_stream::native::NativeAudioStream;
use ::livekit::webrtc::video_frame::{BoxVideoFrame, VideoFormatType};
use ::livekit::webrtc::video_stream::native::NativeVideoStream;
use futures_util::StreamExt;
use jpeg_encoder::{ColorType, Encoder};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::agent::{
    ReportPromptInput, RuntimeState, SpeakerTurn, TEST_REACTION_COOLDOWN_S, TimingInput,
    WATCH_TICK_S, apply_data_event, final_report, format_test_run, log_hint_text, numbered,
    parse_participant_metadata, proactive_review, read_editor_text, report_prompt,
    significant_change, silence_nudge, timing_decision, transcript_for_report, wrap_up,
};
use crate::config::AgentConfig;
use crate::runtime::TOPIC_CONTROL;

/// How long past the interview's nominal duration the agent waits before ending
/// it itself. The browser normally ends the interview; this only has to catch
/// the case where it never does, so it is generous rather than tight.
const INTERVIEW_DEADLINE_GRACE: Duration = Duration::from_secs(120);

/// How long a freshly joined agent waits for its candidate. The agent is now
/// dispatched when `/api/token` names the room, which is before the browser has
/// connected, so this covers the gap; without a bound, every abandoned token
/// request would strand an agent in a room forever.
const CANDIDATE_JOIN_LIMIT: Duration = Duration::from_secs(300);

/// How long the interview survives the candidate leaving. LiveKit only reports
/// a disconnect once its own reconnect window has expired, so this is a second
/// grace on top of that, sized for a page refresh rather than a network blip.
/// Without it a closed tab kept a metered Gemini session open until the
/// interview's full deadline.
const CANDIDATE_ABSENCE_LIMIT: Duration = Duration::from_secs(90);

use crate::gemini::{
    GeminiEvent, GeminiFunctionCall, GeminiLiveSession, generate_report, open_live_session,
    redact_api_key,
};
use crate::runtime::{
    AGENT_NAME, RuntimeBootstrap, TOOL_LOG_HINT, TOOL_READ_EDITOR, TOPIC_REPORT,
    TOPIC_TRANSCRIPTION, agent_identity, bootstrap,
};
use crate::token::{LivekitTokenInput, livekit_room_admin_token, livekit_token};

const GEMINI_AUDIO_SAMPLE_RATE: i32 = 16_000;
const GEMINI_AUDIO_CHANNELS: i32 = 1;
const GEMINI_AUDIO_BUFFER_BYTES: usize = 3_200;
const GEMINI_VIDEO_MIME_TYPE: &str = "image/jpeg";
const GEMINI_VIDEO_FRAME_INTERVAL: Duration = Duration::from_secs(1);
const GEMINI_VIDEO_JPEG_QUALITY: u8 = 75;
const GEMINI_OUTPUT_AUDIO_SAMPLE_RATE: u32 = 24_000;
const LIVEKIT_OUTPUT_CHANNELS: u32 = 1;
const LIVEKIT_OUTPUT_QUEUE_MS: u32 = 100;
const LIVEKIT_OUTPUT_FRAME_QUEUE: usize = 6_000;
const LIVEKIT_AGENT_STATE: &str = "lk.agent.state";
const AGENT_STATE_LISTENING: &str = "listening";
const AGENT_STATE_SPEAKING: &str = "speaking";
const DUPLICATE_AGENT_ISOLATION_ATTEMPTS: usize = 20;
const WRAP_UP_WAIT: Duration = Duration::from_secs(8);
/// Covers `generate_report`'s three attempts and their backoff. The candidate
/// is watching a spinner, so this is the point where waiting stops being worth
/// more than a fallback report.
const REPORT_TIMEOUT: Duration = Duration::from_secs(45);
/// A candidate who has just typed is still working, even if their speech has
/// paused. Give them a beat before a periodic review tries to take the floor.
const CODE_SETTLE: Duration = Duration::from_secs(10);

/// Whether the candidate is in the room, and since when they have not been.
///
/// The departure, the return and the grace check happen in three different
/// arms of the `run_room` loop. They were three lines against one `Option`
/// local, which is to say the rule connecting them was written nowhere and
/// asserted by nothing: ending an interview because somebody's tab reloaded,
/// or keeping a metered Gemini session open because a return never cleared the
/// clock, both looked like ordinary code from any one of the three sites.
///
/// `now` is a parameter rather than an `Instant::now()` inside, so the grace
/// can be tested without waiting ninety seconds for it.
#[derive(Debug, Default)]
struct CandidatePresence {
    left_at: Option<Instant>,
}

impl CandidatePresence {
    fn left(&mut self, now: Instant) {
        self.left_at = Some(now);
    }

    fn returned(&mut self) {
        self.left_at = None;
    }

    /// Present, or absent for less than the grace, both mean carry on.
    fn gave_up(&self, now: Instant) -> bool {
        self.left_at
            .is_some_and(|left| now.duration_since(left) >= CANDIDATE_ABSENCE_LIMIT)
    }
}

/// The three things a data packet has to be before it means anything: sent on a
/// topic, sent by the interview participant, and parseable as JSON.
///
/// Each was a bare `continue` inside the loop, so a packet dropped because it
/// came from a bystander was indistinguishable from one dropped because it was
/// malformed, and nothing covered either. The topic is handed back borrowed
/// because the caller already owns it and the only reason to return it at all
/// is that unwrapping it here is what makes the three guards one decision.
fn interview_packet<'a>(
    topic: Option<&'a str>,
    sender: Option<&str>,
    candidate_identity: &str,
    payload: &[u8],
) -> Option<(&'a str, serde_json::Value)> {
    let topic = topic?;
    if !is_interview_participant(sender, candidate_identity) {
        return None;
    }
    Some((topic, serde_json::from_slice(payload).ok()?))
}

pub async fn run_room(
    config: &AgentConfig,
    room_name: &str,
    now_seconds: u64,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let agent_identity = agent_identity(room_name);
    let (room, mut events) = join_room(config, room_name, &agent_identity, now_seconds).await?;
    let mut agent_state = String::new();
    set_agent_state(&room, &mut agent_state, AGENT_STATE_LISTENING).await?;

    // The candidate's metadata picks the problem, so nothing else can start
    // until someone joins.
    let mut deferred_events = Vec::new();
    let Some((candidate_identity, candidate_metadata)) =
        candidate_metadata(&room, &mut events, &mut deferred_events).await?
    else {
        eprintln!("no candidate joined room={room_name}; leaving it");
        return Ok(());
    };
    let boot = candidate_bootstrap(config, room_name, Some(&candidate_metadata));
    eprintln!(
        "starting interview: room={} problem={} duration={}min",
        boot.room_name, boot.problem.id, boot.duration_min
    );

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
    let (mut output_audio, mut gemini) = tokio::try_join!(
        async {
            isolate_local_agent(config, room_name, &agent_identity, now_seconds).await?;
            publish_output_audio(&room, GEMINI_OUTPUT_AUDIO_SAMPLE_RATE).await
        },
        open_live_session(&config.google_api_key, &boot),
    )?;
    eprintln!(
        "timing: room and Gemini session ready {:.2}s after the candidate joined",
        setup_began.elapsed().as_secs_f64()
    );
    let started_at = Instant::now();
    let interview = InterviewContext {
        config,
        boot: &boot,
        started_at,
    };
    let ids = RoomIdentities {
        room_name,
        agent: &agent_identity,
        candidate: &candidate_identity,
        now_seconds,
    };

    let mut turn = TurnState {
        state: RuntimeState::default(),
        agent_state: std::mem::take(&mut agent_state),
        activity: RuntimeActivity::new(started_at),
        turns: SpeakerTurns::default(),
    };
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
            config.gemini_candidate_video_enabled,
            &event,
        )
        .await?;
    }

    gemini.send_text(&boot.greeting).await?;
    turn.activity.mark_speaking();

    let mut watch = tokio::time::interval(Duration::from_secs_f64(WATCH_TICK_S));

    // Checked on the watch tick rather than given its own timer arm: the tick
    // already runs, and a resolution of one tick is plenty for a ninety-second
    // grace.
    let mut presence = CandidatePresence::default();

    // The interview's own deadline, held by the process that owns the room
    // rather than by the candidate's tab. The browser countdown is a display:
    // it is throttled when the tab is hidden, and it stops entirely on an OS
    // suspend, so an interview whose only clock was the browser could run for
    // as long as the tab stayed open. This is also what bounds a metered Gemini
    // session when a candidate simply closes the tab.
    //
    // The grace is deliberate: the browser normally ends the interview itself,
    // and this only has to catch the case where it never does.
    let hard_deadline = tokio::time::sleep(
        Duration::from_secs(u64::from(boot.duration_min) * 60) + INTERVIEW_DEADLINE_GRACE,
    );
    tokio::pin!(hard_deadline);

    loop {
        tokio::select! {
            () = &mut hard_deadline, if !turn.state.ended => {
                eprintln!(
                    "interview reached its server-side deadline: room={} duration={}min",
                    boot.room_name, boot.duration_min
                );

                // Fed through the same path the browser's own end takes, so the
                // wrap-up, the report and the teardown are the ones that are
                // already tested rather than a second copy that drifts.
                let mut context = turn.context(&mut output_audio, &mut gemini, media.identity.as_deref());

                // The result is always Break for an end_interview packet, and
                // the report has been published by the time it returns.
                let _ = handle_data_packet(
                    &room,
                    &mut context,
                    interview,
                    TOPIC_CONTROL,
                    &serde_json::json!({ "type": "end_interview", "reason": "time_up" }),
                )
                .await?;
                return Ok(());
            }
            _ = watch.tick(), if !turn.state.ended => {
                if presence.gave_up(Instant::now()) {
                    // No report: it would be graded from a session the
                    // candidate walked out of, and there is nobody in the room
                    // to receive it. The browser writes the report the
                    // candidate actually sees.
                    eprintln!("candidate did not return to room={room_name}; ending");
                    gemini.close().await?;
                    close_room(&room).await;
                    return Ok(());
                }
                if let Some(prompt) = turn.activity.watch_prompt(&turn.state, Instant::now()) {
                    gemini.send_text(&prompt).await?;
                    turn.activity.mark_speaking();
                }
            }
            event = events.recv() => {
                let Some(event) = event else {
                    eprintln!("LiveKit event stream ended for room={room_name}; ending");
                    gemini.close().await?;
                    return Ok(());
                };
                if handle_media_event(
                    &mut media,
                    &mut gemini,
                    &candidate_identity,
                    config.gemini_candidate_video_enabled,
                    &event,
                )
                .await?
                {
                    continue;
                }
                let mut context = turn.context(&mut output_audio, &mut gemini, media.identity.as_deref());
                if handle_room_event(&room, &mut context, &mut presence, interview, &ids, event)
                    .await?
                    .is_break()
                {
                    return Ok(());
                }
            }
            event = gemini.next_event() => {
                let Some(event) = event else {
                    // The interview ends here whenever Gemini hangs up, which
                    // from the candidate's side is the interviewer stopping
                    // mid-sentence with the transcript cut at the same word.
                    // The reason came over the socket and is logged by the
                    // reader task; this line is what ties that to the room.
                    eprintln!(
                        "Gemini session ended; ending interview room={room_name} and letting the supervisor restart"
                    );
                    close_room(&room).await;
                    return Ok(());
                };
                let mut context = turn.context(&mut output_audio, &mut gemini, media.identity.as_deref());
                handle_gemini_event(&room, &mut context, event, Interruptible::Yes).await?;
            }
            _ = tokio::time::sleep_until(output_audio.playout_deadline.into()), if turn.activity.floor == Floor::AwaitingPlayout && output_audio.is_playing() => {
                if output_audio.is_playing() {
                    continue;
                }
                turn.activity.mark_listening();
                set_agent_state(&room, &mut turn.agent_state, AGENT_STATE_LISTENING).await?;
            }
            frame = next_audio_frame(&mut media.audio), if media.audio.is_some() => {
                pump_audio(&mut media, &mut gemini, frame).await?;
            }
            frame = next_video_frame(&mut media.video), if media.video.is_some() => {
                pump_video(&mut media, &mut gemini, frame).await?;
            }
        }
    }
}

/// Buffers one arriving audio frame, and flushes when there is enough to send.
///
/// `None` is the track ending: what is buffered goes now, because nothing else
/// is going to arrive to push it over the threshold.
async fn pump_audio(
    media: &mut CandidateMedia,
    gemini: &mut GeminiLiveSession,
    frame: Option<AudioFrame<'static>>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let Some(frame) = frame else {
        flush_audio(gemini, &mut media.audio_bytes).await?;
        media.audio = None;
        return Ok(());
    };

    // Not `last_user_speech`. LiveKit delivers frames for as long as the track
    // is live, silence included, so stamping here made the interview
    // permanently "just spoke": `idle_seconds` never reached
    // SILENCE_THRESHOLD_S and the nudge never fired. Real speech is stamped on
    // `InputTranscript`, where Gemini has already decided that words were said.
    append_pcm16_bytes(&frame, &mut media.audio_bytes);
    if media.audio_bytes.len() >= GEMINI_AUDIO_BUFFER_BYTES {
        flush_audio(gemini, &mut media.audio_bytes).await?;
    }
    Ok(())
}

/// Sends one arriving video frame, at most one per interval.
async fn pump_video(
    media: &mut CandidateMedia,
    gemini: &mut GeminiLiveSession,
    frame: Option<BoxVideoFrame>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let Some(frame) = frame else {
        media.video = None;
        return Ok(());
    };
    if !should_send_video_frame(media.last_video_frame.elapsed()) {
        return Ok(());
    }

    // Stamped on the attempt, not the success: the dimensions come off the
    // wire, and a source stuck emitting frames that will not encode must cost
    // one try per interval rather than one per arriving frame.
    media.last_video_frame = Instant::now();
    match encode_video_frame_jpeg(&frame, GEMINI_VIDEO_JPEG_QUALITY) {
        Ok(bytes) => {
            gemini
                .send_video_frame(&bytes, GEMINI_VIDEO_MIME_TYPE)
                .await?
        }
        Err(error) => eprintln!("skipping unencodable video frame: {error}"),
    }
    Ok(())
}

/// One LiveKit room event that was not the candidate's media.
///
/// `Break` means the interview is over: the browser asked to end it, and the
/// report has already been published by the time this returns.
async fn handle_room_event(
    room: &Room,
    context: &mut GeminiEventContext<'_>,
    presence: &mut CandidatePresence,
    interview: InterviewContext<'_>,
    ids: &RoomIdentities<'_>,
    event: RoomEvent,
) -> Result<ControlFlow<()>, Box<dyn std::error::Error + Send + Sync>> {
    let RoomIdentities {
        room_name,
        agent: agent_identity,
        candidate: candidate_identity,
        now_seconds,
    } = *ids;
    let config = interview.config;
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
            let Some((topic, payload)) = interview_packet(
                topic.as_deref(),
                sender.as_deref(),
                candidate_identity,
                &payload,
            ) else {
                return Ok(ControlFlow::Continue(()));
            };
            if handle_data_packet(room, context, interview, topic, &payload)
                .await?
                .is_break()
            {
                // The browser asking to end is the ordinary way an
                // interview finishes, and it was the one exit that
                // said nothing. Indistinguishable in the log from
                // the agent dying and being restarted under it.
                eprintln!("interview ended by the browser: room={room_name} topic={topic}");
                return Ok(ControlFlow::Break(()));
            }
        }

        // The server's own word for why the agent is going away.
        // Without it the only trace is the event channel closing a
        // moment later, which says a disconnect happened and
        // nothing about whose fault it was: a duplicate identity, a
        // deleted room and a signal drop all look identical from
        // there, and they need three different fixes.
        //
        // `DuplicateIdentity` is the one worth naming outright. The
        // agent identity is derived from the room name, so a second
        // agent process on the same room is not a near-miss, it is
        // the same string, and the server evicts whichever joined
        // first. `is_duplicate_agent` cannot see that case: it
        // matches on the identity being different.
        RoomEvent::Disconnected { reason } => {
            if reason == DisconnectReason::DuplicateIdentity {
                eprintln!(
                    "another agent joined room={room_name} as {agent_identity} and took the session; this one is a second agent process on the same room"
                );
            } else {
                eprintln!("disconnected from room={room_name}: {reason:?}");
            }
        }
        RoomEvent::ParticipantConnected(participant)
            if is_duplicate_agent(&participant, agent_identity) =>
        {
            evict_duplicate_agent(config, room_name, &participant, now_seconds).await?;
        }
        RoomEvent::ParticipantDisconnected(participant)
            if participant.identity().0 == candidate_identity =>
        {
            eprintln!("candidate left room={room_name}; waiting for a return");
            presence.left(Instant::now());
        }
        RoomEvent::ParticipantConnected(participant)
            if participant.identity().0 == candidate_identity =>
        {
            presence.returned();
        }
        _ => {}
    }
    Ok(ControlFlow::Continue(()))
}

/// Mints the agent token and connects. Split out so `run_room` reads as the
/// interview loop rather than the LiveKit handshake.
async fn join_room(
    config: &AgentConfig,
    room_name: &str,
    agent_identity: &str,
    now_seconds: u64,
) -> Result<
    (Room, tokio::sync::mpsc::UnboundedReceiver<RoomEvent>),
    Box<dyn std::error::Error + Send + Sync>,
> {
    let token = livekit_token(LivekitTokenInput {
        api_key: &config.livekit_api_key,
        api_secret: &config.livekit_api_secret,
        name: AGENT_NAME,
        identity: agent_identity,
        room: room_name,
        metadata: "{}",
        now_seconds,
        agent: true,
    })?;
    let (room, events) = Room::connect(&config.livekit_url, &token, RoomOptions::default()).await?;
    eprintln!(
        "joined room={} identity={}",
        room_name,
        room.local_participant().identity()
    );
    Ok((room, events))
}

/// The candidate's inbound media, plus the identity their tracks arrived on.
/// Bundled because every track event touches two or three of these together.
struct CandidateMedia {
    audio: Option<NativeAudioStream>,
    video: Option<NativeVideoStream>,
    audio_bytes: Vec<u8>,
    identity: Option<String>,
    last_video_frame: Instant,
}

impl CandidateMedia {
    fn new() -> Self {
        Self {
            audio: None,
            video: None,
            audio_bytes: Vec::new(),
            identity: None,

            // Seeded in the past so the first frame sends immediately. The
            // monotonic clock starts at boot, so on a machine that just came up
            // there may be nothing to subtract from.
            last_video_frame: Instant::now()
                .checked_sub(GEMINI_VIDEO_FRAME_INTERVAL)
                .unwrap_or_else(Instant::now),
        }
    }

    fn attach_audio(&mut self, track: &RemoteAudioTrack, participant: &RemoteParticipant) {
        // Drop any half-buffered frame from a previous publication so the new
        // stream does not start mid-sample.
        self.audio_bytes.clear();
        self.identity = Some(participant.identity().to_string());
        self.audio = Some(NativeAudioStream::new(
            track.rtc_track(),
            GEMINI_AUDIO_SAMPLE_RATE,
            GEMINI_AUDIO_CHANNELS,
        ));
    }
}

struct RuntimeActivity {
    last_code_change: Instant,
    last_user_speech: Instant,
    /// When the candidate stopped talking and started waiting, if they have and
    /// the reply has not begun. Separate from `last_user_speech`, which the
    /// nudge logic needs seeded at the interview start and never absent.
    ///
    /// Sharing one field is what made the reply-latency line lie. `Interrupted`
    /// hands the floor back without ever saying when the candidate finished, so
    /// a reply that followed one measured from whatever the seed was: on the
    /// first turn that is the start of the interview, which is how a candidate
    /// who waited under a second was reported as having waited fifteen.
    awaiting_reply_since: Option<Instant>,
    last_agent_speech: Instant,
    last_nudge: Instant,
    last_review: Instant,
    last_interjection: Instant,
    last_test_reaction: Instant,
    code_at_last_review: String,
    floor: Floor,
}

/// Who holds the conversation. `agent_busy` + `agent_turn_complete` encoded
/// this as two bools, but only three of the four combinations were reachable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Floor {
    /// The candidate has the floor.
    Listening,
    /// A prompt is in flight; Gemini is still producing the turn.
    Speaking,
    /// The turn is complete but queued audio is still draining.
    AwaitingPlayout,
}

impl RuntimeActivity {
    fn new(now: Instant) -> Self {
        Self {
            last_code_change: now,
            last_user_speech: now,
            awaiting_reply_since: None,
            last_agent_speech: now,
            last_nudge: now,
            last_review: now,
            last_interjection: now,

            // Seeded in the past so the first test run reacts immediately; see
            // `CandidateMedia::new` for why the subtraction is checked.
            last_test_reaction: now
                .checked_sub(Duration::from_secs_f64(TEST_REACTION_COOLDOWN_S))
                .unwrap_or(now),
            code_at_last_review: String::new(),
            floor: Floor::Listening,
        }
    }

    /// The agent has the floor: it was just handed a prompt and owns the
    /// conversation until Gemini reports the turn complete.
    fn mark_speaking(&mut self) {
        self.floor = Floor::Speaking;
    }

    /// Turn finished and audio drained: hand the floor back to the candidate.
    fn mark_listening(&mut self) {
        self.floor = Floor::Listening;
        self.last_agent_speech = Instant::now();
    }

    /// Gemini has transcribed something the candidate said, so they have
    /// stopped talking and started waiting.
    ///
    /// The two stamps are not the same fact, which is why they are separate
    /// fields. `last_user_speech` feeds the idle timers and has to move on
    /// every fragment. `awaiting_reply_since` is the start of a measurable
    /// wait, and it exists only while there is a wait to measure.
    ///
    /// Not armed while the agent holds the floor. Input transcription lags the
    /// audio it describes, so a fragment covering the tail of what the
    /// candidate said can land after the reply has already begun. Arming on
    /// that one made the next chunk of a turn already in progress announce
    /// itself as the reply starting, measured from a moment nobody waited from.
    fn note_candidate_finished(&mut self, now: Instant) {
        self.last_user_speech = now;
        if self.floor != Floor::Speaking {
            self.awaiting_reply_since = Some(now);
        }
    }

    /// `now` is passed in rather than sampled here, like every other method on
    /// this struct. Sampling internally makes the boundaries untestable: a test
    /// can set `last_code_change` to exactly `CODE_SETTLE` ago, but the clock
    /// read inside would already have moved past it, so a half-open window and
    /// a closed one behave identically to every test that can be written.
    fn watch_prompt(&mut self, state: &RuntimeState, now: Instant) -> Option<String> {
        let decision = timing_decision(&TimingInput {
            agent_busy: self.floor != Floor::Listening,
            user_talking: now.duration_since(self.last_code_change) < CODE_SETTLE,
            idle_seconds: now
                .duration_since(
                    self.last_user_speech
                        .max(self.last_code_change)
                        .max(self.last_agent_speech),
                )
                .as_secs_f64(),
            since_last_nudge_seconds: now.duration_since(self.last_nudge).as_secs_f64(),
            since_last_review_seconds: now.duration_since(self.last_review).as_secs_f64(),
            since_last_interjection_seconds: now
                .duration_since(self.last_interjection)
                .as_secs_f64(),
            speech_gap_seconds: now
                .duration_since(self.last_user_speech.max(self.last_agent_speech))
                .as_secs_f64(),
            significant_change: significant_change(&self.code_at_last_review, &state.code),
        });
        if decision.update_last_nudge {
            self.last_nudge = now;
        }
        if decision.update_last_review {
            self.last_review = now;
        }
        if decision.update_last_interjection {
            self.last_interjection = now;
        }
        if decision.sync_code_at_last_review {
            self.code_at_last_review = state.code.clone();
        }
        if decision.silence_nudge {
            return Some(silence_nudge(&numbered(&state.code)));
        }
        if decision.proactive_review {
            return Some(proactive_review(&numbered(&state.code)));
        }
        None
    }
}

async fn isolate_local_agent(
    config: &AgentConfig,
    room_name: &str,
    local_identity: &str,
    now_seconds: u64,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // One list pass more than removal rounds: the last pass only confirms the
    // final removal landed.
    let mut rounds_left = DUPLICATE_AGENT_ISOLATION_ATTEMPTS;
    loop {
        let participants = list_room_participants(config, room_name, now_seconds).await?;
        let duplicate_agents = duplicate_agent_identities(&participants, local_identity);
        if duplicate_agents.is_empty() {
            return Ok(());
        }
        if rounds_left == 0 {
            // Loud, not fatal. This used to return Err, and in `serve` an agent
            // error aborts the web task and exits the process, so one stubborn
            // auto-dispatched agent took the whole server down mid-interview.
            // Two interviewers talking over each other is bad; no interviewer,
            // no editor and no report is worse, and the operator can see this
            // line and act on it while the candidate finishes.
            eprintln!(
                "WARNING: duplicate LiveKit agent participants remained after \
                 {DUPLICATE_AGENT_ISOLATION_ATTEMPTS} attempts, continuing anyway: \
                 {duplicate_agents:?}"
            );
            return Ok(());
        }
        rounds_left -= 1;
        for identity in &duplicate_agents {
            eprintln!("removing duplicate LiveKit agent participant identity={identity}");
            remove_room_participant(config, room_name, identity, now_seconds).await?;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

async fn list_room_participants(
    config: &AgentConfig,
    room_name: &str,
    now_seconds: u64,
) -> Result<serde_json::Value, Box<dyn std::error::Error + Send + Sync>> {
    livekit_room_service_request(
        config,
        room_name,
        "ListParticipants",
        serde_json::json!({ "room": room_name }),
        now_seconds,
    )
    .await
}

/// A removal that finds nothing to remove has achieved what it was asked to do.
///
/// This used to propagate the 404 and kill the interview. The duplicate agents
/// being evicted here are the ones that leave on their own, so losing the race
/// with them is the common case, not the exceptional one: the agent greeted the
/// candidate, tried to evict a participant that had already gone, took the
/// `not_found` as fatal, and exited. The candidate was left in a room with no
/// interviewer, talking to nobody, with the status pill correctly reading
/// "Waiting" and nothing on screen explaining why.
async fn remove_room_participant(
    config: &AgentConfig,
    room_name: &str,
    identity: &str,
    now_seconds: u64,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    match livekit_room_service_request(
        config,
        room_name,
        "RemoveParticipant",
        serde_json::json!({ "room": room_name, "identity": identity }),
        now_seconds,
    )
    .await
    {
        Ok(_) => Ok(()),
        Err(error) if is_participant_gone(&error.to_string()) => {
            eprintln!("duplicate LiveKit agent participant already gone identity={identity}");
            Ok(())
        }
        Err(error) => Err(error),
    }
}

/// Matches on the wire response rather than on a status code alone, because
/// LiveKit answers Twirp over HTTP and a bare 404 could also mean the route is
/// wrong. All three parts are required: the method, so a 404 from any other
/// RoomService call stays fatal; the status; and `not_found` in the body, which
/// is the participant-specific Twirp code rather than an HTML error page.
fn is_participant_gone(error: &str) -> bool {
    error.contains("RemoveParticipant") && error.contains("404") && error.contains("not_found")
}

async fn livekit_room_service_request(
    config: &AgentConfig,
    room_name: &str,
    method: &str,
    body: serde_json::Value,
    now_seconds: u64,
) -> Result<serde_json::Value, Box<dyn std::error::Error + Send + Sync>> {
    let token = livekit_room_admin_token(
        &config.livekit_api_key,
        &config.livekit_api_secret,
        room_name,
        now_seconds,
    )?;
    let url = format!(
        "{}/twirp/livekit.RoomService/{method}",
        livekit_http_base(&config.livekit_url)
    );
    let response = crate::http_client()
        .post(url)
        .bearer_auth(token)
        .json(&body)
        .send()
        .await?;
    let status = response.status();
    let text = response.text().await?;
    if !status.is_success() {
        return Err(format!("LiveKit RoomService {method} failed: {status} {text}").into());
    }
    parse_room_service_response(method, &text)
}

fn livekit_http_base(url: &str) -> String {
    url.trim_end_matches('/')
        .replacen("wss://", "https://", 1)
        .replacen("ws://", "http://", 1)
}

fn parse_room_service_response(
    method: &str,
    text: &str,
) -> Result<serde_json::Value, Box<dyn std::error::Error + Send + Sync>> {
    serde_json::from_str(text).map_err(|error| {
        format!("LiveKit RoomService {method} returned invalid JSON: {error}").into()
    })
}

fn duplicate_agent_identities(
    participants: &serde_json::Value,
    local_identity: &str,
) -> Vec<String> {
    participants
        .get("participants")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter(|participant| is_agent_participant(participant))
        .filter_map(|participant| {
            participant
                .get("identity")
                .and_then(serde_json::Value::as_str)
        })
        .filter(|identity| *identity != local_identity)
        .map(ToString::to_string)
        .collect()
}

fn is_agent_participant(participant: &serde_json::Value) -> bool {
    participant
        .pointer("/permission/agent")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
        || participant
            .get("kind")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|kind| kind == "AGENT")
}

fn is_candidate(participant: &RemoteParticipant) -> bool {
    participant.kind() == ParticipantKind::Standard
        && candidate_identity_matches(&participant.identity().0, &participant.metadata())
}

pub fn candidate_identity_matches(identity: &str, metadata: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(metadata)
        .ok()
        .and_then(|metadata| {
            metadata
                .get("candidateIdentity")?
                .as_str()
                .map(str::to_owned)
        })
        .is_some_and(|minted| minted == identity && minted.starts_with("candidate-"))
}

/// A second agent in the room would answer over the top of this one, so the
/// late joiner is removed rather than tolerated.
async fn evict_duplicate_agent(
    config: &AgentConfig,
    room_name: &str,
    participant: &RemoteParticipant,
    now_seconds: u64,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let identity = participant.identity().0;
    eprintln!("removing late duplicate LiveKit agent participant identity={identity}");
    remove_room_participant(config, room_name, &identity, now_seconds).await
}

/// The candidate's tracks arriving and going away. Reports whether the event
/// was media, so the caller's match only has to carry what is left.
async fn handle_media_event(
    media: &mut CandidateMedia,
    gemini: &mut GeminiLiveSession,
    candidate_identity: &str,
    forward_video_to_gemini: bool,
    event: &RoomEvent,
) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
    match event {
        RoomEvent::TrackSubscribed {
            track: RemoteTrack::Audio(track),
            participant,
            ..
        } if is_interview_participant(Some(&participant.identity().0), candidate_identity) => {
            media.attach_audio(track, participant);
        }
        RoomEvent::TrackSubscribed {
            track: RemoteTrack::Video(track),
            participant,
            ..
        } if candidate_video_frames_go_to_gemini(
            Some(&participant.identity().0),
            candidate_identity,
            forward_video_to_gemini,
        ) =>
        {
            media.video = Some(NativeVideoStream::new(track.rtc_track()));
        }

        // Guarded like the subscribe arms: an unrelated participant leaving
        // must not tear down the stream the candidate is still speaking into.
        RoomEvent::TrackUnsubscribed {
            track: RemoteTrack::Audio(_),
            participant,
            ..
        } if is_interview_participant(Some(&participant.identity().0), candidate_identity) => {
            flush_audio(gemini, &mut media.audio_bytes).await?;
            media.audio = None;
        }
        RoomEvent::TrackUnsubscribed {
            track: RemoteTrack::Video(_),
            participant,
            ..
        } if is_interview_participant(Some(&participant.identity().0), candidate_identity) => {
            media.video = None;
        }
        _ => return Ok(false),
    }
    Ok(true)
}

fn candidate_video_frames_go_to_gemini(
    sender: Option<&str>,
    candidate_identity: &str,
    opt_in: bool,
) -> bool {
    opt_in && is_interview_participant(sender, candidate_identity)
}

/// Only the candidate this interview bootstrapped from may drive the runtime.
/// Their token grants both `canPublishData` and `canPublish`, so a second
/// participant in the room could otherwise end the interview over the data
/// channel or feed their own microphone to the interviewer over a track.
///
/// An unattributed event is refused too. The SDK reports `None` when the sender
/// is not a remote participant it knows, which for this agent means a
/// server-side publish, and nothing here sends itself packets. Accepting them
/// would buy nothing and leave a hole to argue about.
fn is_interview_participant(sender: Option<&str>, candidate_identity: &str) -> bool {
    sender.is_some_and(|sender| sender == candidate_identity)
}

fn is_duplicate_agent(participant: &RemoteParticipant, local_identity: &str) -> bool {
    participant.kind() == ParticipantKind::Agent
        && participant.identity().to_string() != local_identity
}

async fn close_room(room: &Room) {
    if let Err(error) = room.close().await {
        eprintln!("room close ignored: {error}");
    }
}

/// What the interview needs from whoever joined. Assembled in one place because
/// the candidate can be found two ways, already in the room or arriving, and
/// the pair must not be built differently depending on which.
fn candidate_pair(participant: &RemoteParticipant) -> (String, String) {
    (participant.identity().to_string(), participant.metadata())
}

/// `Ok(None)` when nobody arrived within [`CANDIDATE_JOIN_LIMIT`]. A no-show is
/// not a failure, so it does not spend a restart attempt; the agent simply
/// leaves the room rather than holding it open for a candidate who closed the
/// tab between requesting a token and connecting.
/// Events seen while waiting are handed back in `deferred` rather than dropped.
/// A `TrackSubscribed` consumed here used to be gone for good, and the only
/// thing that attaches the candidate's microphone is that event, so losing it
/// left an interview where the agent never heard anything at all.
async fn candidate_metadata(
    room: &Room,
    events: &mut tokio::sync::mpsc::UnboundedReceiver<RoomEvent>,
    deferred: &mut Vec<RoomEvent>,
) -> Result<Option<(String, String)>, Box<dyn std::error::Error + Send + Sync>> {
    if let Some(participant) = room
        .remote_participants()
        .values()
        .find(|participant| is_candidate(participant))
    {
        return Ok(Some(candidate_pair(participant)));
    }
    eprintln!("waiting for candidate in room={}", room.name());
    let deadline = tokio::time::Instant::now() + CANDIDATE_JOIN_LIMIT;
    loop {
        match tokio::time::timeout_at(deadline, events.recv()).await {
            Err(_) => return Ok(None),
            Ok(Some(RoomEvent::ParticipantConnected(participant)))
                if is_candidate(&participant) =>
            {
                return Ok(Some(candidate_pair(&participant)));
            }
            Ok(Some(other)) => {
                // The wait has its own event loop, so a disconnect here never
                // reaches the one in `run_room`. Passing it on in silence is
                // how this ends as the bare "room closed before a candidate
                // joined" below, which names the symptom and not one of the
                // several unrelated causes: a duplicate identity, a deleted
                // room and a dropped signal socket all arrive here looking
                // identical, and they need three different fixes.
                if let RoomEvent::Disconnected { reason } = &other {
                    eprintln!(
                        "disconnected from room={} while waiting for a candidate: {reason:?}",
                        room.name()
                    );
                }
                deferred.push(other);
            }
            Ok(None) => return Err("room closed before a candidate joined".into()),
        }
    }
}

fn candidate_bootstrap<'a>(
    config: &'a AgentConfig,
    room_name: &'a str,
    metadata: Option<&str>,
) -> RuntimeBootstrap<'a> {
    let candidate = parse_participant_metadata(metadata);
    bootstrap(
        config,
        room_name,
        Some(candidate.problem.id),
        candidate.duration_min,
    )
}

async fn flush_audio(
    gemini: &mut GeminiLiveSession,
    bytes: &mut Vec<u8>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if !bytes.is_empty() {
        gemini.send_audio_pcm_16khz(bytes).await?;
        bytes.clear();
    }
    Ok(())
}

async fn next_audio_frame(stream: &mut Option<NativeAudioStream>) -> Option<AudioFrame<'static>> {
    stream.as_mut()?.next().await
}

async fn next_video_frame(stream: &mut Option<NativeVideoStream>) -> Option<BoxVideoFrame> {
    stream.as_mut()?.next().await
}

/// The immutable side of a running interview: fixed once the candidate joins,
/// and needed only when the interview ends and the report is written.
#[derive(Clone, Copy)]
/// The identities this room was joined with, fixed for the life of the
/// interview. Grouped because deciding whose event just arrived needs several
/// of them at once, and passing them one at a time made the signature of every
/// handler a list of four strings whose order was the only thing keeping them
/// apart.
struct RoomIdentities<'a> {
    room_name: &'a str,
    agent: &'a str,
    candidate: &'a str,
    now_seconds: u64,
}

#[derive(Clone, Copy)]
struct InterviewContext<'a> {
    config: &'a AgentConfig,
    boot: &'a RuntimeBootstrap<'a>,
    started_at: Instant,
}

/// Applies one decoded data packet. `Break` means the interview is over and the
/// report has been published.
async fn handle_data_packet(
    room: &Room,
    context: &mut GeminiEventContext<'_>,
    interview: InterviewContext<'_>,
    topic: &str,
    payload: &serde_json::Value,
) -> Result<ControlFlow<()>, Box<dyn std::error::Error + Send + Sync>> {
    let result = apply_data_event(
        context.state,
        topic,
        payload,
        context.activity.last_test_reaction.elapsed().as_secs_f64(),
    );
    if result.update_last_code_change {
        context.activity.last_code_change = Instant::now();
    }
    if result.update_last_test_reaction {
        context.activity.last_test_reaction = Instant::now();
    }
    if result.update_last_interjection {
        context.activity.last_interjection = Instant::now();
    }
    if let Some(prompt) = result.generate_reply {
        context.gemini.send_text(&prompt).await?;
        context.activity.mark_speaking();
    }
    let Some(reason) = result.finish_interview else {
        return Ok(ControlFlow::Continue(()));
    };

    if should_send_wrap_up(&reason) {
        send_wrap_up_and_wait(room, context, &reason).await?;
    }

    // The goodbye is the last turn there will be. Closing here keeps the panel
    // from leaving it rendered as still-in-progress for the rest of the page's
    // life, and costs one publish.
    close_turns(room, context).await?;
    publish_report(
        room,
        interview.boot,
        context.state,
        &reason,
        interview.started_at.elapsed().as_secs_f64() / 60.0,
        &interview.config.google_api_key,
    )
    .await?;
    context.gemini.shutdown().await?;
    // Give the report packet a moment to leave before tearing the room down.
    tokio::time::sleep(Duration::from_millis(250)).await;
    close_room(room).await;
    Ok(ControlFlow::Break(()))
}

/// What one interview accumulates, minus the two things the select loop borrows
/// as futures. `gemini` and `media` have to stay outside: their arms hold a
/// mutable borrow for as long as the select is polled, so bundling them here
/// would make every other arm's condition fight the borrow checker.
///
/// The four that are left exist to make [`TurnState::context`] possible. Three
/// arms of the select need a `GeminiEventContext`, and it borrows seven things
/// mutably, so it lives exactly as long as the call it is handed to. That used
/// to be a macro, purely to stop the field list being written out three times.
///
/// Worth knowing before anyone tries to go further: bundling these four and
/// stopping there makes the function *longer*, because every use site grows a
/// prefix and the macro stays. It only pays once the method exists to replace
/// the macro outright.
struct TurnState {
    state: RuntimeState,
    agent_state: String,
    activity: RuntimeActivity,
    turns: SpeakerTurns,
}

impl TurnState {
    fn context<'a>(
        &'a mut self,
        output_audio: &'a mut OutputAudio,
        gemini: &'a mut GeminiLiveSession,
        candidate_identity: Option<&'a str>,
    ) -> GeminiEventContext<'a> {
        GeminiEventContext {
            output_audio,
            gemini,
            state: &mut self.state,
            agent_state: &mut self.agent_state,
            activity: &mut self.activity,
            turns: &mut self.turns,
            candidate_identity,
        }
    }
}

struct GeminiEventContext<'a> {
    output_audio: &'a mut OutputAudio,
    gemini: &'a mut GeminiLiveSession,
    state: &'a mut RuntimeState,
    agent_state: &'a mut String,
    activity: &'a mut RuntimeActivity,
    turns: &'a mut SpeakerTurns,
    candidate_identity: Option<&'a str>,
}

/// Whether a candidate talking over the interviewer cuts it short.
///
/// Carried as an argument rather than a flag on the context. It was a field
/// that `send_wrap_up_and_wait` set and restored, which is one early return
/// away from leaving barge-in off for good, and it read as state when it is
/// really a property of the turn being handled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Interruptible {
    /// An ordinary exchange. The candidate speaking wins the floor.
    Yes,
    /// The closing message. Cutting it leaves the candidate without the ending,
    /// and `wrap_up_settled` reads the emptied queue as the turn being over, so
    /// a "thanks" mid sentence ended the interview there.
    No,
}

/// One open turn per speaker. Gemini interleaves input and output
/// transcription, so they cannot share a single accumulator.
#[derive(Debug, Default)]
struct SpeakerTurns {
    interviewer: SpeakerTurn,
    candidate: SpeakerTurn,
}

async fn send_wrap_up_and_wait(
    room: &Room,
    context: &mut GeminiEventContext<'_>,
    reason: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    context.gemini.send_text(&wrap_up(reason)).await?;
    context.activity.mark_speaking();
    let deadline = Instant::now() + WRAP_UP_WAIT;
    loop {
        if Instant::now() >= deadline {
            return Ok(());
        }
        if wrap_up_settled(context.output_audio, context.activity) {
            context.activity.mark_listening();
            set_agent_state(room, context.agent_state, AGENT_STATE_LISTENING).await?;
            return Ok(());
        }
        tokio::select! {
            event = context.gemini.next_event() => {
                let Some(event) = event else { return Ok(()) };
                // The closing message is the one turn that plays to the end.
                handle_gemini_event(room, context, event, Interruptible::No).await?;
            }
            _ = tokio::time::sleep_until(context.output_audio.playout_deadline.into()), if context.activity.floor == Floor::AwaitingPlayout => {}
            _ = tokio::time::sleep_until(deadline.into()) => {
                return Ok(());
            }
        }
    }
}

/// The goodbye is done once nothing is queued and Gemini is not mid-turn.
fn wrap_up_settled(output_audio: &OutputAudio, activity: &RuntimeActivity) -> bool {
    !output_audio.is_playing() && activity.floor != Floor::Speaking
}

/// Throws away agent audio that is queued but no longer wanted.
///
/// Only acts in `Floor::AwaitingPlayout` with audio genuinely still queued.
/// `AwaitingPlayout` alone is not enough: it is stamped once when the turn
/// completes, and the queue drains on its own afterwards, so the state outlives
/// the condition it was named for.
///
/// Deliberately does not call `close_turns`. `TurnComplete` already closed both
/// sides before setting this floor, and closing again would split one utterance
/// across two transcript segments.
async fn drop_stale_playout(
    room: &Room,
    context: &mut GeminiEventContext<'_>,
    interruptible: Interruptible,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let Some(dropped) = take_stale_playout(context.activity, context.output_audio, interruptible)
    else {
        return Ok(());
    };
    eprintln!(
        "timing: dropped {:.1}s of queued interviewer speech the candidate talked over",
        dropped.as_secs_f64()
    );
    set_agent_state(room, context.agent_state, AGENT_STATE_LISTENING).await?;
    Ok(())
}

/// The decision and its effect, with no room in sight so a test can reach it.
/// Returns how much queued speech was thrown away, which both decides whether
/// the agent state attribute needs republishing and is the number worth
/// logging: it is exactly the delay the candidate would otherwise have sat
/// through before hearing an answer.
/// Ends a turn that will not finish: drops what is queued and hands the floor
/// back. Returns how much speech was thrown away.
///
/// `mark_listening`, not a bare assignment to the floor. This used to assign it
/// bare, on the reasoning that stamping agent speech would start the silence
/// timers from the wrong instant. It does the opposite: `last_agent_speech` is
/// parked at the playout deadline while audio is queued, and that deadline is
/// in the future, so leaving it there after throwing the queue away suppresses
/// the silence nudge for the whole length of speech nobody heard. `now` is the
/// earlier of the two.
fn cut_off_turn(activity: &mut RuntimeActivity, output_audio: &mut OutputAudio) -> Duration {
    let unplayed = output_audio
        .playout_deadline
        .saturating_duration_since(Instant::now());
    output_audio.interrupt();
    activity.mark_listening();

    // The pending measurement dies with the turn. A candidate who finished,
    // waited, and then started talking again is no longer waiting for anything,
    // and leaving the stamp behind meant the next reply that produced audio
    // without its own transcript measured from it: the same lie this field was
    // split out of `last_user_speech` to stop telling, one turn later.
    activity.awaiting_reply_since = None;
    unplayed
}

fn take_stale_playout(
    activity: &mut RuntimeActivity,
    output_audio: &mut OutputAudio,
    interruptible: Interruptible,
) -> Option<Duration> {
    if interruptible == Interruptible::No
        || activity.floor != Floor::AwaitingPlayout
        || !output_audio.is_playing()
    {
        return None;
    }
    Some(cut_off_turn(activity, output_audio))
}

async fn handle_gemini_event(
    room: &Room,
    context: &mut GeminiEventContext<'_>,
    event: GeminiEvent,
    interruptible: Interruptible,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    match event {
        GeminiEvent::ToolCall(calls) => {
            for call in calls {
                let response = execute_tool_call(context.state, &call);
                context.gemini.send_tool_response(&call, response).await?;
            }
        }
        GeminiEvent::OutputTranscript(text) => {
            // `text` is passed whole, not the trimmed form: fragments have to
            // be concatenated exactly as received. The trimmed view only
            // decides whether this event carried anything at all.
            if transcript_text(&text).is_some() {
                let turn = &mut context.turns.interviewer;
                let whole = turn
                    .record(&mut context.state.transcript, "Interviewer", &text)
                    .to_string();
                publish_transcript(room, &whole, turn.segment_id("interviewer"), false, None)
                    .await?;
            }
        }
        GeminiEvent::InputTranscript(text) => {
            if let (Some(_), Some(identity)) = (transcript_text(&text), context.candidate_identity)
            {
                // The candidate is talking over audio Gemini finished producing
                // a while ago. Gemini will not call this an interruption,
                // because as far as it is concerned that turn ended when it
                // stopped generating; only this side knows the queue is still
                // draining. Cut it here or the reply lands behind the rest of
                // the old turn.
                drop_stale_playout(room, context, interruptible).await?;
                context.activity.note_candidate_finished(Instant::now());
                let turn = &mut context.turns.candidate;
                let whole = turn
                    .record(&mut context.state.transcript, "Candidate", &text)
                    .to_string();
                publish_transcript(
                    room,
                    &whole,
                    turn.segment_id("candidate"),
                    false,
                    Some(identity),
                )
                .await?;
            }
        }
        GeminiEvent::Audio { bytes, mime_type } => {
            // Read before the interrupt below, because that is the point the
            // candidate stops waiting. Stamped on the last input transcript
            // fragment, so it covers endpointing plus model latency plus
            // anything still queued ahead of the reply: the silence the
            // candidate actually sits through.
            //
            // `None` means Gemini is answering something it never transcribed,
            // and there is no moment the candidate finished to measure from.
            // Printing anything then is worse than printing nothing.
            let waited = context.activity.awaiting_reply_since;

            // A new turn's first chunk while the previous one is still
            // draining. `InputTranscript` normally clears the queue before this
            // point, so reaching here means Gemini answered something it never
            // transcribed. Backstop rather than the main path, and it must run
            // before `capture` or the new audio queues behind the old.
            //
            // Only for a chunk that will actually be queued. Dropping ahead of
            // a chunk `capture` rejects leaves the candidate with a sentence
            // cut in half and no reply behind it.
            if context.output_audio.accepts(&bytes, &mime_type) {
                drop_stale_playout(room, context, interruptible).await?;
            }
            if context.output_audio.capture(&bytes, &mime_type).await? {
                // Cleared here rather than where it is read: a chunk `capture`
                // rejects is not the reply starting, and consuming the stamp on
                // one would lose the measurement for the chunk that is.
                if let Some(since) = waited {
                    context.activity.awaiting_reply_since = None;
                    eprintln!(
                        "timing: {:.2}s from the candidate finishing to the reply starting",
                        since.elapsed().as_secs_f64()
                    );
                }
                context.activity.mark_speaking();

                // Speech is queued, not played: the floor stays busy until the
                // buffered audio actually finishes.
                context.activity.last_agent_speech = context.output_audio.playout_deadline;
                set_agent_state(room, context.agent_state, AGENT_STATE_SPEAKING).await?;
            }
        }
        GeminiEvent::TurnComplete => {
            // The gap between Gemini finishing and the queue emptying. Gemini
            // synthesises far faster than speech plays, so this is how long the
            // agent will still be talking after it has stopped thinking, and
            // therefore how long a candidate answering now would have waited
            // before `drop_stale_playout` existed.
            let backlog = context
                .output_audio
                .playout_deadline
                .saturating_duration_since(Instant::now());
            if !backlog.is_zero() {
                eprintln!(
                    "timing: turn generated, {:.1}s of it still to play",
                    backlog.as_secs_f64()
                );
            }

            // Gemini finishing its turn also means the candidate utterance it
            // answered is over, so both sides close here.
            close_turns(room, context).await?;
            context.activity.floor = Floor::AwaitingPlayout;
            if !context.output_audio.is_playing() {
                context.activity.mark_listening();
                set_agent_state(room, context.agent_state, AGENT_STATE_LISTENING).await?;
            }
        }
        GeminiEvent::Interrupted => {
            // The only other path that empties the queue, and it used to do so
            // silently. If a turn is cut this way the candidate hears a
            // fragment or nothing, and without this line the log shows only the
            // consequence: a turn that completed with nothing left to play.
            let unplayed = cut_off_turn(context.activity, context.output_audio);
            eprintln!(
                "timing: Gemini cut its own turn, {:.1}s of it unplayed",
                unplayed.as_secs_f64()
            );

            // A cut-off turn is still over. Without this the next thing either
            // party says appends to the abandoned turn under its segment id, so
            // the panel would glue two separate utterances into one row and the
            // report prompt would read them as one line.
            close_turns(room, context).await?;

            set_agent_state(room, context.agent_state, AGENT_STATE_LISTENING).await?;
        }
        _ => {}
    }
    Ok(())
}

async fn set_agent_state(
    room: &Room,
    current: &mut String,
    next: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if current == next {
        return Ok(());
    }
    let participant = room.local_participant();
    participant
        .set_attributes(agent_state_attributes(participant.attributes(), next))
        .await?;
    current.clear();
    current.push_str(next);
    Ok(())
}

fn agent_state_attributes(
    mut attributes: HashMap<String, String>,
    state: &str,
) -> HashMap<String, String> {
    attributes.insert(LIVEKIT_AGENT_STATE.to_string(), state.to_string());
    attributes
}

struct OutputAudio {
    source: NativeAudioSource,
    sample_rate: u32,
    pending_bytes: Vec<u8>,
    playout_deadline: Instant,
    frames: mpsc::Sender<QueuedOutputFrame>,
    output_cancellation: CancellationToken,
}

struct QueuedOutputFrame {
    output_cancellation: CancellationToken,
    samples: Vec<i16>,
}

impl OutputAudio {
    fn interrupt(&mut self) {
        self.output_cancellation.cancel();
        self.output_cancellation = CancellationToken::new();
        self.source.clear_buffer();
        self.pending_bytes.clear();
        self.playout_deadline = Instant::now();
    }

    fn is_playing(&self) -> bool {
        Instant::now() < self.playout_deadline
    }

    /// Whether `capture` would queue anything for this chunk.
    ///
    /// Split out so the caller can ask before dropping a turn that is still
    /// playing. Dropping first and then finding the new chunk unusable cuts the
    /// interviewer off mid-sentence with nothing behind it.
    fn accepts(&self, bytes: &[u8], mime_type: &str) -> bool {
        !bytes.is_empty()
            && audio_sample_rate(mime_type, self.sample_rate) == Some(self.sample_rate)
    }

    async fn capture(
        &mut self,
        bytes: &[u8],
        mime_type: &str,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        if !self.accepts(bytes, mime_type) {
            return Ok(false);
        }
        let mut queued = false;
        for samples in take_pcm16_frames(
            bytes,
            self.sample_rate,
            LIVEKIT_OUTPUT_CHANNELS,
            &mut self.pending_bytes,
        ) {
            queued = true;
            let samples_per_channel = samples.len() as u32 / LIVEKIT_OUTPUT_CHANNELS;
            let frame_duration =
                Duration::from_secs_f64(samples_per_channel as f64 / self.sample_rate as f64);
            self.playout_deadline = self.playout_deadline.max(Instant::now()) + frame_duration;
            self.frames
                .try_send(QueuedOutputFrame {
                    output_cancellation: self.output_cancellation.clone(),
                    samples,
                })
                .map_err(|error| {
                    std::io::Error::other(match error {
                        mpsc::error::TrySendError::Full(_) => "output audio worker queue full",
                        mpsc::error::TrySendError::Closed(_) => "output audio worker stopped",
                    })
                })?;
        }
        Ok(queued)
    }
}

async fn output_audio_worker(
    source: NativeAudioSource,
    sample_rate: u32,
    mut frames: mpsc::Receiver<QueuedOutputFrame>,
) {
    while let Some(frame) = frames.recv().await {
        if frame.output_cancellation.is_cancelled() {
            continue;
        }
        let cancelled = frame.output_cancellation.cancelled();
        tokio::pin!(cancelled);
        let samples_per_channel = frame.samples.len() as u32 / LIVEKIT_OUTPUT_CHANNELS;
        let audio_frame = AudioFrame {
            data: frame.samples.into(),
            sample_rate,
            num_channels: LIVEKIT_OUTPUT_CHANNELS,
            samples_per_channel,
        };
        tokio::select! {
            result = source.capture_frame(&audio_frame) => {
                if let Err(error) = result {
                    eprintln!("failed to publish output audio frame: {error}");
                }
            }
            _ = &mut cancelled => {
                source.clear_buffer();
            }
        }
    }
}

async fn publish_output_audio(
    room: &Room,
    sample_rate: u32,
) -> Result<OutputAudio, Box<dyn std::error::Error + Send + Sync>> {
    let source = NativeAudioSource::new(
        AudioSourceOptions::default(),
        sample_rate,
        LIVEKIT_OUTPUT_CHANNELS,
        LIVEKIT_OUTPUT_QUEUE_MS,
    );
    let track = LocalAudioTrack::create_audio_track(
        "interviewer-audio",
        RtcAudioSource::Native(source.clone()),
    );
    room.local_participant()
        .publish_track(
            LocalTrack::Audio(track),
            TrackPublishOptions {
                source: TrackSource::Microphone,
                ..Default::default()
            },
        )
        .await?;
    let (frames, queued_frames) = mpsc::channel(LIVEKIT_OUTPUT_FRAME_QUEUE);
    tokio::spawn(output_audio_worker(
        source.clone(),
        sample_rate,
        queued_frames,
    ));
    Ok(OutputAudio {
        source,
        sample_rate,
        pending_bytes: Vec::new(),
        playout_deadline: Instant::now(),
        frames,
        output_cancellation: CancellationToken::new(),
    })
}

fn execute_tool_call(state: &mut RuntimeState, call: &GeminiFunctionCall) -> serde_json::Value {
    match call.name.as_str() {
        TOOL_READ_EDITOR => serde_json::json!({
            "result": read_editor_text(
                &state.language,
                &state.code,
                state.last_test_run.as_ref(),
                state.test_runs,
            )
        }),
        TOOL_LOG_HINT => {
            state.hints_used += 1;
            serde_json::json!({ "result": log_hint_text(state.hints_used) })
        }
        name => serde_json::json!({ "error": format!("unknown tool: {name}") }),
    }
}

/// Republishes each open turn once as final, so the browser can stop showing
/// it as in-progress, then opens fresh segment ids for the next turn.
async fn close_turns(
    room: &Room,
    context: &mut GeminiEventContext<'_>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let candidate_identity = context.candidate_identity.map(str::to_string);
    let closing = [
        close_turn(&mut context.turns.interviewer, "interviewer"),
        close_turn(&mut context.turns.candidate, "candidate"),
    ];
    for (index, closed) in closing.into_iter().enumerate() {
        let Some((whole, segment_id)) = closed else {
            continue;
        };

        // Only the candidate's own speech is attributed to them; the
        // interviewer publishes as the agent participant.
        let identity = (index == 1)
            .then_some(candidate_identity.as_deref())
            .flatten();
        publish_transcript(room, &whole, segment_id, true, identity).await?;
    }
    Ok(())
}

/// Ends the turn and hands back what has to be published as final, or `None`
/// when the speaker had nothing open. Split out so the borrow ends before the
/// publish await.
fn close_turn(turn: &mut SpeakerTurn, speaker: &str) -> Option<(String, String)> {
    if !turn.is_open() {
        return None;
    }
    let closed = (turn.text().to_string(), turn.segment_id(speaker));
    turn.finish();
    Some(closed)
}

async fn publish_transcript(
    room: &Room,
    text: &str,
    segment_id: String,
    final_segment: bool,
    sender_identity: Option<&str>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    room.local_participant()
        .send_text(
            text,
            transcript_stream_options(segment_id, final_segment, sender_identity),
        )
        .await?;
    Ok(())
}

/// `lk.segment_id` is what lets the browser patch one row per turn instead of
/// reassembling sentences from timestamps, and `lk.transcription_final` says
/// whether the turn is still being spoken.
fn transcript_stream_options(
    segment_id: String,
    final_segment: bool,
    sender_identity: Option<&str>,
) -> StreamTextOptions {
    let options = StreamTextOptions::new_with_topic(TOPIC_TRANSCRIPTION)
        .with_attribute("lk.segment_id", &segment_id)
        .with_attribute(
            "lk.transcription_final",
            if final_segment { "true" } else { "false" },
        );
    match sender_identity {
        Some(identity) => options.with_sender_identity(identity),
        None => options,
    }
}

fn transcript_text(text: &str) -> Option<&str> {
    let text = text.trim();
    (!text.is_empty()).then_some(text)
}

fn append_pcm16_bytes(frame: &AudioFrame<'_>, bytes: &mut Vec<u8>) {
    frame
        .data
        .iter()
        .for_each(|sample| bytes.extend(sample.to_le_bytes()));
}

fn audio_sample_rate(mime_type: &str, default_pcm_rate: u32) -> Option<u32> {
    mime_type
        .split(';')
        .find_map(|part| {
            part.trim()
                .strip_prefix("rate=")
                .and_then(|rate| rate.parse().ok())
        })
        .or_else(|| (mime_type.trim() == "audio/pcm").then_some(default_pcm_rate))
}

fn take_pcm16_frames(
    bytes: &[u8],
    sample_rate: u32,
    channels: u32,
    pending: &mut Vec<u8>,
) -> Vec<Vec<i16>> {
    let frame_bytes = (sample_rate / 100 * channels) as usize * 2;
    if frame_bytes == 0 {
        return Vec::new();
    }

    pending.extend_from_slice(bytes);
    let complete_len = pending.len() - pending.len() % frame_bytes;
    let frames = pending[..complete_len]
        .chunks_exact(frame_bytes)
        .map(decode_pcm16)
        .collect();
    pending.drain(..complete_len);
    frames
}

fn decode_pcm16(bytes: &[u8]) -> Vec<i16> {
    bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|chunk| i16::from_le_bytes([chunk[0], chunk[1]]))
        .collect()
}

fn should_send_video_frame(elapsed: Duration) -> bool {
    elapsed >= GEMINI_VIDEO_FRAME_INTERVAL
}

/// JPEG sides and RGBA byte count for a frame, or `None` when the dimensions
/// are absurd. Both bounds live here because they answer one question and a
/// frame that clears the pixel cap can still overflow a 16-bit JPEG side: the
/// pixel cap is well past 8K and only exists so a bogus header cannot ask for a
/// gigabyte, while a 100000x1 frame sits under it and still has no valid SOF.
fn jpeg_frame_geometry(width: u32, height: u32) -> Option<(u16, u16, usize)> {
    const MAX_PIXELS: u32 = 8192 * 8192;
    let pixels = width.checked_mul(height)?;
    (1..=MAX_PIXELS).contains(&pixels).then_some(())?;
    Some((
        u16::try_from(width).ok()?,
        u16::try_from(height).ok()?,
        pixels as usize * 4,
    ))
}

/// Converts a frame to packed R, G, B, A bytes.
///
/// The format name is a trap. libyuv names its 32-bit formats after the
/// little-endian word rather than the byte order, so `ABGR` is the variant that
/// writes R, G, B, A into memory and `RGBA` writes A, B, G, R. Asking for
/// `RGBA` pins the red channel to the alpha constant and transposes green and
/// blue, which `channel_order_survives_the_libyuv_naming_trap` pins down.
fn frame_to_rgba(frame: &BoxVideoFrame) -> Result<(Vec<u8>, u16, u16), String> {
    let width = frame.buffer.as_ref().width();
    let height = frame.buffer.as_ref().height();

    // Remote-controlled dimensions: overflow here would panic in debug and
    // under-allocate the buffer `to_argb` writes into in release.
    let Some((jpeg_width, jpeg_height, rgba_len)) = jpeg_frame_geometry(width, height) else {
        return Err(format!(
            "frame {width}x{height} is outside JPEG limits (each side at most 65535, at most 8192x8192 pixels)"
        ));
    };
    let mut rgba = vec![0; rgba_len];
    frame.buffer.as_ref().to_argb(
        VideoFormatType::ABGR,
        &mut rgba,
        width * 4,
        width as i32,
        height as i32,
    );
    Ok((rgba, jpeg_width, jpeg_height))
}

fn encode_video_frame_jpeg(frame: &BoxVideoFrame, quality: u8) -> Result<Vec<u8>, String> {
    let (rgba, jpeg_width, jpeg_height) = frame_to_rgba(frame)?;

    // Chroma is subsampled 2x2 here, because that is what `Encoder::new` picks
    // below quality 90 and nothing below overrides it. Deliberate: the source
    // is I420, whose chroma is already 4:2:0, so encoding it at full resolution
    // would spend about a fifth of the payload on interpolated values. The
    // override, if a future frame source is not I420, is
    // `set_sampling_factor(SamplingFactor::F_1_1)` before the call.
    let mut jpeg = Vec::new();
    Encoder::new(&mut jpeg, quality)
        .encode(&rgba, jpeg_width, jpeg_height, ColorType::Rgba)
        .map_err(|error| error.to_string())?;
    Ok(jpeg)
}

async fn publish_report(
    room: &Room,
    boot: &RuntimeBootstrap<'_>,
    state: &RuntimeState,
    reason: &str,
    elapsed_min: f64,
    api_key: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    room.local_participant()
        .publish_data(report_packet(boot, state, reason, elapsed_min, api_key).await?)
        .await?;
    Ok(())
}

async fn report_packet(
    boot: &RuntimeBootstrap<'_>,
    state: &RuntimeState,
    reason: &str,
    elapsed_min: f64,
    api_key: &str,
) -> Result<DataPacket, Box<dyn std::error::Error + Send + Sync>> {
    let report = match tokio::time::timeout(
        REPORT_TIMEOUT,
        generate_report(
            api_key,
            boot.report_model,
            &report_prompt_text(boot, state, elapsed_min),
        ),
    )
    .await
    {
        Ok(Ok(raw)) => final_report(Some(&raw), state.hints_used, None),
        Ok(Err(error)) => final_report(
            None,
            state.hints_used,
            Some(&report_error_note(
                boot,
                state,
                reason,
                error.as_ref(),
                api_key,
            )),
        ),
        Err(error) => final_report(
            None,
            state.hints_used,
            Some(&report_error_note(boot, state, reason, &error, api_key)),
        ),
    };
    Ok(report_data_packet(report_with_integrity_events(
        report, state,
    ))?)
}

fn report_with_integrity_events(
    mut report: serde_json::Value,
    state: &RuntimeState,
) -> serde_json::Value {
    if let Some(object) = report.as_object_mut() {
        object.insert(
            "integrityEvents".to_string(),
            serde_json::Value::Array(state.integrity_events.clone()),
        );
    }
    report
}

fn should_send_wrap_up(reason: &str) -> bool {
    reason != "candidate_ended"
}

fn report_prompt_text(
    boot: &RuntimeBootstrap<'_>,
    state: &RuntimeState,
    elapsed_min: f64,
) -> String {
    let transcript = transcript_for_report(&state.transcript);
    let test_summary = format_test_run(state.last_test_run.as_ref(), state.test_runs);
    report_prompt(ReportPromptInput {
        problem: boot.problem,
        transcript: &transcript,
        final_code: &state.code,
        language: &state.language,
        hints_used: state.hints_used,
        duration_min: boot.duration_min,
        elapsed_min,
        test_summary: &test_summary,
    })
}

/// This note is published to the candidate's browser and rendered in the report
/// card, so `api_key` is not decoration: an error carrying a credentialed URL
/// would otherwise hand the server's Google key to whoever is taking the
/// interview.
fn report_error_note(
    boot: &RuntimeBootstrap<'_>,
    state: &RuntimeState,
    reason: &str,
    error: &(dyn std::error::Error + 'static),
    api_key: &str,
) -> String {
    let detail = redact_api_key(&error.to_string(), api_key);
    format!(
        "Rust LiveKit runner ended ({reason}) but Gemini report generation failed for model {} on {}. Final editor state: {} bytes of {}. Error: {detail}",
        boot.report_model,
        boot.problem.id,
        state.code.len(),
        state.language
    )
}

fn report_data_packet(report: serde_json::Value) -> Result<DataPacket, serde_json::Error> {
    Ok(DataPacket {
        payload: serde_json::to_vec(&report)?,
        topic: Some(TOPIC_REPORT.to_string()),
        reliable: true,
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::load_from_pairs;
    use ::livekit::webrtc::video_frame::{I420Buffer, VideoBuffer, VideoFrame, VideoRotation};

    #[test]
    fn only_the_interview_candidate_can_drive_the_runtime() {
        assert!(is_interview_participant(
            Some("candidate-abc"),
            "candidate-abc"
        ));

        // A peer sharing the room must not be able to end the interview,
        // rewrite the editor, or speak to the interviewer as the candidate.
        assert!(!is_interview_participant(
            Some("candidate-xyz"),
            "candidate-abc"
        ));
        assert!(
            !is_interview_participant(None, "candidate-abc"),
            "an unattributed event drives nothing: this agent never sends itself packets"
        );
    }

    /// The three guards a data packet passes before it can drive anything, in
    /// one place because they used to be three `continue`s that looked alike.
    #[test]
    fn only_the_candidates_parseable_packet_on_a_topic_drives_the_runtime() {
        let good = br#"{"type":"end_interview"}"#;

        let (topic, payload) = interview_packet(
            Some(TOPIC_CONTROL),
            Some("candidate-abc"),
            "candidate-abc",
            good,
        )
        .expect("the candidate's own JSON on a topic is the whole point");
        assert_eq!(topic, TOPIC_CONTROL);
        assert_eq!(payload["type"], "end_interview");

        // LiveKit makes the topic optional, and every handler downstream keys
        // off it, so a packet without one is addressed to nothing.
        assert!(interview_packet(None, Some("candidate-abc"), "candidate-abc", good).is_none());

        // The interesting half of the gate: a bystander in the room must not be
        // able to end the interview or rewrite the editor.
        assert!(
            interview_packet(
                Some(TOPIC_CONTROL),
                Some("candidate-xyz"),
                "candidate-abc",
                good
            )
            .is_none()
        );
        assert!(interview_packet(Some(TOPIC_CONTROL), None, "candidate-abc", good).is_none());

        // Dropped rather than propagated: the sender is the browser we ship, so
        // a body that will not parse is a bug to fix rather than an interview
        // to end under the candidate.
        assert!(
            interview_packet(
                Some(TOPIC_CONTROL),
                Some("candidate-abc"),
                "candidate-abc",
                b"not json",
            )
            .is_none()
        );
    }

    /// Ending an interview because a tab reloaded, and keeping a metered Gemini
    /// session open because a return never cleared the clock, are the two ways
    /// this goes wrong. Both used to be spread across three `select!` arms with
    /// nothing asserting they agreed.
    #[test]
    fn the_absence_grace_starts_on_departure_and_a_return_clears_it() {
        let start = Instant::now();
        let mut presence = CandidatePresence::default();

        // Nobody has left, so no amount of elapsed time ends anything.
        assert!(!presence.gave_up(start + CANDIDATE_ABSENCE_LIMIT * 10));

        presence.left(start);
        assert!(
            !presence.gave_up(start),
            "the grace starts, it does not expire"
        );
        assert!(
            !presence.gave_up(start + CANDIDATE_ABSENCE_LIMIT - Duration::from_secs(1)),
            "a page refresh is inside the grace and must not end the interview"
        );

        // The boundary is the tick the watch would fire on, so it has to be
        // inclusive: an exclusive one waits an extra whole tick.
        assert!(presence.gave_up(start + CANDIDATE_ABSENCE_LIMIT));

        presence.returned();
        assert!(
            !presence.gave_up(start + CANDIDATE_ABSENCE_LIMIT * 10),
            "coming back cancels the grace outright rather than pausing it"
        );
    }

    /// The note goes over the data channel into the candidate's report card. A
    /// `reqwest` error Displays the URL it was built from, so a credentialed
    /// URL
    /// anywhere in that chain hands the server's Google key to the candidate.
    #[test]
    fn report_error_note_never_carries_the_google_api_key() {
        let config = load_from_pairs([
            ("LIVEKIT_URL", "wss://example.livekit.cloud"),
            ("LIVEKIT_API_KEY", "devkey"),
            ("LIVEKIT_API_SECRET", "devsecret"),
            ("GOOGLE_API_KEY", "AQ.Ab8RN6secret"),
        ])
        .unwrap();
        let boot = bootstrap(&config, "interview-fixed", Some("two-sum"), 45);
        let leaky = std::io::Error::other(
            "HTTP status server error (503 Service Unavailable) for url \
             (https://generativelanguage.googleapis.com/v1beta/models/x:generateContent?key=AQ.Ab8RN6secret)",
        );

        let note = report_error_note(
            &boot,
            &RuntimeState::default(),
            "candidate_ended",
            &leaky,
            "AQ.Ab8RN6secret",
        );

        assert!(!note.contains("AQ.Ab8RN6secret"), "{note}");
        assert!(note.contains("[REDACTED]"), "{note}");
        assert!(
            note.contains("503 Service Unavailable"),
            "the reason still has to be readable: {note}"
        );
    }

    #[test]
    fn jpeg_frame_geometry_rejects_overflowing_and_empty_frames() {
        assert_eq!(
            jpeg_frame_geometry(640, 480),
            Some((640, 480, 640 * 480 * 4))
        );
        assert_eq!(
            jpeg_frame_geometry(8192, 8192),
            Some((8192, 8192, 8192 * 8192 * 4))
        );

        // Would wrap a u32 multiply and under-allocate the buffer `to_argb`
        // writes into.
        assert_eq!(jpeg_frame_geometry(u32::MAX, 4), None);
        assert_eq!(jpeg_frame_geometry(8193, 8192), None);
        assert_eq!(jpeg_frame_geometry(0, 480), None);

        // Clears the pixel cap and still has no representable JPEG side.
        assert_eq!(jpeg_frame_geometry(100_000, 1), None);

        // The exact boundary, in both directions, because 65535 is the largest
        // side a JPEG SOF can carry and off-by-one here is a silent truncation.
        assert_eq!(
            jpeg_frame_geometry(65_535, 1),
            Some((65_535, 1, 65_535 * 4))
        );
        assert_eq!(jpeg_frame_geometry(65_536, 1), None);
        assert_eq!(jpeg_frame_geometry(1, 65_536), None);
    }

    #[test]
    fn candidate_bootstrap_uses_participant_metadata() {
        let config = load_from_pairs([
            ("LIVEKIT_URL", "wss://example.livekit.cloud"),
            ("LIVEKIT_API_KEY", "devkey"),
            ("LIVEKIT_API_SECRET", "devsecret"),
            ("GOOGLE_API_KEY", "google-key"),
        ])
        .unwrap();

        let boot = candidate_bootstrap(
            &config,
            "interview-fixed",
            Some(r#"{"problemId":"merge-intervals","durationMin":30}"#),
        );

        assert_eq!(boot.problem.id, "merge-intervals");
        assert_eq!(boot.duration_min, 30);
    }

    #[test]
    fn candidate_video_frames_are_opt_in_for_gemini() {
        let default_config = load_from_pairs([
            ("LIVEKIT_URL", "wss://example.livekit.cloud"),
            ("LIVEKIT_API_KEY", "devkey"),
            ("LIVEKIT_API_SECRET", "devsecret"),
            ("GOOGLE_API_KEY", "google-key"),
        ])
        .unwrap();
        let enabled_config = load_from_pairs([
            ("LIVEKIT_URL", "wss://example.livekit.cloud"),
            ("LIVEKIT_API_KEY", "devkey"),
            ("LIVEKIT_API_SECRET", "devsecret"),
            ("GOOGLE_API_KEY", "google-key"),
            ("CODETRIAL_GEMINI_CANDIDATE_VIDEO_ENABLED", "true"),
        ])
        .unwrap();

        assert!(
            !candidate_video_frames_go_to_gemini(
                Some("candidate-fixed"),
                "candidate-fixed",
                default_config.gemini_candidate_video_enabled,
            ),
            "default integrity mode must not forward candidate video frames server-side"
        );
        assert!(candidate_video_frames_go_to_gemini(
            Some("candidate-fixed"),
            "candidate-fixed",
            enabled_config.gemini_candidate_video_enabled,
        ));
        assert!(!candidate_video_frames_go_to_gemini(
            Some("candidate-other"),
            "candidate-fixed",
            enabled_config.gemini_candidate_video_enabled,
        ));
    }

    /// Losing the race with a duplicate agent that left on its own must not end
    /// the interview. This shipped as a fatal error: the agent greeted the
    /// candidate, tried to evict a participant that was already gone, and
    /// exited on the `not_found`, leaving the candidate talking to an empty
    /// room. Every other RoomService failure stays fatal.
    #[test]
    fn a_removal_that_finds_nobody_is_not_a_failure() {
        assert!(is_participant_gone(
            "LiveKit RoomService RemoveParticipant failed: 404 Not Found {\"code\":\"not_found\",\"msg\":\"participant does not exist\"}"
        ));

        for still_fatal in [
            "LiveKit RoomService RemoveParticipant failed: 401 Unauthorized {\"code\":\"unauthenticated\"}",
            "LiveKit RoomService RemoveParticipant failed: 503 Service Unavailable {}",
            "LiveKit RoomService ListParticipants failed: 404 Not Found <html>no such route</html>",
        ] {
            assert!(!is_participant_gone(still_fatal), "{still_fatal}");
        }
    }

    #[test]
    fn duplicate_agent_identities_ignores_candidate_and_local_agent() {
        let participants = serde_json::json!({
            "participants": [
                {
                    "identity": "candidate-fixed",
                    "kind": "STANDARD",
                    "permission": { "agent": false }
                },
                {
                    "identity": "interviewer-interview-fixed",
                    "kind": "AGENT",
                    "permission": { "agent": true }
                },
                {
                    "identity": "hosted-agent",
                    "kind": "AGENT",
                    "permission": { "agent": true }
                },
                {
                    "identity": "legacy-agent",
                    "permission": { "agent": true }
                }
            ]
        });

        assert_eq!(
            duplicate_agent_identities(&participants, "interviewer-interview-fixed"),
            vec!["hosted-agent".to_string(), "legacy-agent".to_string()]
        );
    }

    #[test]
    fn livekit_http_base_maps_websocket_urls_to_room_service_host() {
        assert_eq!(
            livekit_http_base("wss://example.livekit.cloud/"),
            "https://example.livekit.cloud"
        );
        assert_eq!(
            livekit_http_base("ws://localhost:7880"),
            "http://localhost:7880"
        );
    }

    #[test]
    fn room_service_response_rejects_malformed_success_json() {
        let error = parse_room_service_response("ListParticipants", "not json")
            .unwrap_err()
            .to_string();

        assert!(error.contains("ListParticipants returned invalid JSON"));
    }

    #[test]
    fn report_helpers_use_report_topic_prompt_state_and_error_note() {
        let config = load_from_pairs([
            ("LIVEKIT_URL", "wss://example.livekit.cloud"),
            ("LIVEKIT_API_KEY", "devkey"),
            ("LIVEKIT_API_SECRET", "devsecret"),
            ("GOOGLE_API_KEY", "google"),
        ])
        .unwrap();
        let boot = bootstrap(&config, "interview-fixed", Some("two-sum"), 45);
        let state = RuntimeState {
            code: "return [0, 1]".to_string(),
            language: "python".to_string(),
            transcript: vec![
                "Interviewer: Welcome.".to_string(),
                "Candidate: I will use a hash map.".to_string(),
            ],
            last_test_run: Some(serde_json::json!({
                "language": "python",
                "passed": 1,
                "total": 2,
                "failures": [],
            })),
            test_runs: 1,
            hints_used: 2,
            ..RuntimeState::default()
        };
        let error = std::io::Error::other("model unavailable");
        let report = final_report(
            None,
            state.hints_used,
            Some(&report_error_note(
                &boot,
                &state,
                "candidate_ended",
                &error,
                "google",
            )),
        );
        let packet = report_data_packet(report).unwrap();
        let payload: serde_json::Value = serde_json::from_slice(&packet.payload).unwrap();
        let report = report_with_integrity_events(
            payload.clone(),
            &RuntimeState {
                integrity_events: vec![serde_json::json!({
                    "type": "SESSION_START",
                    "at": "now",
                    "severity": "info",
                    "source": "media",
                    "durationMs": 0,
                    "seq": 1,
                    "prevHash": "",
                    "hash": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                    "detail": null,
                })],
                ..RuntimeState::default()
            },
        );
        let prompt = report_prompt_text(&boot, &state, 12.4);

        assert!(prompt.contains("Candidate: I will use a hash map."));
        assert!(prompt.contains("Latest test run (run #1, python): 1/2 cases passed."));
        assert!(packet.reliable);
        assert_eq!(packet.topic.as_deref(), Some(TOPIC_REPORT));
        assert!(
            payload["summary"]
                .as_str()
                .unwrap()
                .contains("Final editor state: 13 bytes of python")
        );
        assert_eq!(payload["hintsUsed"], 2);
        assert_eq!(report["integrityEvents"][0]["type"], "SESSION_START");
    }

    #[test]
    fn execute_tool_call_reads_editor_and_tracks_hints() {
        let mut state = RuntimeState {
            code: "def two_sum(nums, target):\n    return [0, 1]".to_string(),
            language: "python".to_string(),
            last_test_run: Some(serde_json::json!({
                "language": "python",
                "passed": 1,
                "total": 2,
                "failures": [],
            })),
            test_runs: 1,
            ..RuntimeState::default()
        };

        let editor = execute_tool_call(
            &mut state,
            &GeminiFunctionCall {
                id: "1".to_string(),
                name: TOOL_READ_EDITOR.to_string(),
                args: serde_json::json!({}),
            },
        );
        let hint = execute_tool_call(
            &mut state,
            &GeminiFunctionCall {
                id: "2".to_string(),
                name: TOOL_LOG_HINT.to_string(),
                args: serde_json::json!({}),
            },
        );

        assert!(
            editor["result"]
                .as_str()
                .unwrap()
                .contains("Editor language: python")
        );
        assert!(
            editor["result"]
                .as_str()
                .unwrap()
                .contains("Latest test run")
        );
        assert_eq!(hint["result"], "Recorded. Total hints so far: 1.");
        assert_eq!(state.hints_used, 1);
    }

    #[test]
    fn execute_tool_call_reports_unknown_tools() {
        let mut state = RuntimeState::default();

        assert_eq!(
            execute_tool_call(
                &mut state,
                &GeminiFunctionCall {
                    id: "1".to_string(),
                    name: "missing".to_string(),
                    args: serde_json::json!({}),
                },
            ),
            serde_json::json!({"error":"unknown tool: missing"})
        );
    }

    #[test]
    fn runtime_activity_emits_periodic_prompts_and_updates_gates() {
        let now = Instant::now();
        let mut activity = RuntimeActivity::new(now);
        let mut state = RuntimeState {
            code: "def two_sum(nums, target):\n    return []".to_string(),
            ..RuntimeState::default()
        };
        activity.last_code_change = now - Duration::from_secs(16);
        activity.last_user_speech = now - Duration::from_secs(16);
        activity.last_agent_speech = now - Duration::from_secs(16);
        activity.last_nudge = now - Duration::from_secs(31);

        let prompt = activity.watch_prompt(&state, now).unwrap();

        assert!(prompt.contains("silent AND has not typed"));
        assert!(activity.watch_prompt(&state, now).is_none());

        activity.last_code_change = now - CODE_SETTLE - Duration::from_secs(1);
        activity.last_user_speech = now - Duration::from_secs(5);
        activity.last_agent_speech = now - Duration::from_secs(5);
        activity.last_review = now - Duration::from_secs(31);
        activity.last_interjection = now - Duration::from_secs(46);
        state
            .code
            .push_str("\nseen = {}\nfor i, n in enumerate(nums):\n    pass");

        let prompt = activity.watch_prompt(&state, now).unwrap();

        assert!(prompt.contains("Periodic editor snapshot"));
        assert_eq!(activity.code_at_last_review, state.code);
        assert!(activity.watch_prompt(&state, now).is_none());
    }

    /// The point of CODE_SETTLE. Without it the whole gate can be reverted to a
    /// bare `false` and every other test still passes: the periodic review is
    /// the only caller that can fire while the candidate is mid-edit.
    #[test]
    fn recent_typing_holds_off_the_periodic_review() {
        let now = Instant::now();
        let mut activity = RuntimeActivity::new(now);
        let mut state = RuntimeState {
            code: "def two_sum(nums, target):\n    return []".to_string(),
            ..RuntimeState::default()
        };
        // Everything a proactive review needs is satisfied except the settle.
        activity.last_user_speech = now - Duration::from_secs(5);
        activity.last_agent_speech = now - Duration::from_secs(5);
        activity.last_review = now - Duration::from_secs(31);
        activity.last_interjection = now - Duration::from_secs(46);
        state
            .code
            .push_str("\nseen = {}\nfor i, n in enumerate(nums):\n    pass");

        activity.last_code_change = now - CODE_SETTLE + Duration::from_secs(1);
        assert!(
            activity.watch_prompt(&state, now).is_none(),
            "a candidate who typed a second ago is still working; the review must wait"
        );

        // Exactly CODE_SETTLE has settled: the window is half-open. Without
        // this the boundary is only bracketed, and `<` reads the same as `<=`.
        activity.last_code_change = now - CODE_SETTLE;
        let prompt = activity
            .watch_prompt(&state, now)
            .expect("an edit exactly CODE_SETTLE old has settled; the review may take the floor");
        assert!(prompt.contains("Periodic editor snapshot"));
    }

    #[test]
    fn candidate_exit_skips_wrap_up_before_report() {
        assert!(!should_send_wrap_up("candidate_ended"));
        assert!(should_send_wrap_up("time_up"));
    }

    #[test]
    fn wrap_up_wait_finishes_after_turn_and_playout_complete() {
        let now = Instant::now();
        let (mut output_audio, _) = test_output_audio(Vec::new());
        let mut activity = RuntimeActivity::new(now);
        let drained = now - Duration::from_secs(1);
        let playing = now + Duration::from_secs(1);

        // Idle and nothing queued: the goodbye is already over.
        assert!(wrap_up_settled(&output_audio, &activity));

        // Mid-turn always waits, queued audio or not.
        activity.floor = Floor::Speaking;
        output_audio.playout_deadline = drained;
        assert!(!wrap_up_settled(&output_audio, &activity));
        output_audio.playout_deadline = playing;
        assert!(!wrap_up_settled(&output_audio, &activity));

        // Turn produced, audio still draining: wait for the buffer.
        activity.floor = Floor::AwaitingPlayout;
        assert!(!wrap_up_settled(&output_audio, &activity));

        output_audio.playout_deadline = drained;
        assert!(wrap_up_settled(&output_audio, &activity));

        // Barge-in hands the floor back with audio cleared.
        activity.floor = Floor::Listening;
        assert!(wrap_up_settled(&output_audio, &activity));
    }

    #[test]
    fn floor_transitions_track_who_is_talking() {
        let now = Instant::now();
        let mut activity = RuntimeActivity::new(now);

        assert_eq!(activity.floor, Floor::Listening);

        activity.mark_speaking();
        assert_eq!(activity.floor, Floor::Speaking);

        activity.floor = Floor::AwaitingPlayout;
        activity.mark_listening();
        assert_eq!(activity.floor, Floor::Listening);
        assert!(activity.last_agent_speech >= now);
    }

    /// The browser patches one row per segment id, so an in-progress turn has
    /// to carry the same id it will carry when it closes.
    #[test]
    fn transcript_stream_options_carry_a_segment_id_and_final_flag() {
        let open = transcript_stream_options("interviewer-0".to_string(), false, None);

        assert_eq!(open.topic, TOPIC_TRANSCRIPTION);
        assert_eq!(
            open.attributes.get("lk.segment_id"),
            Some(&"interviewer-0".to_string())
        );
        assert_eq!(
            open.attributes.get("lk.transcription_final"),
            Some(&"false".to_string())
        );
        assert_eq!(open.sender_identity, None);

        let closed = transcript_stream_options("interviewer-0".to_string(), true, None);

        assert_eq!(
            closed.attributes.get("lk.segment_id"),
            Some(&"interviewer-0".to_string()),
            "closing a turn must not change the row it patches"
        );
        assert_eq!(
            closed.attributes.get("lk.transcription_final"),
            Some(&"true".to_string())
        );
    }

    #[test]
    fn transcript_stream_options_can_preserve_candidate_identity() {
        let options =
            transcript_stream_options("candidate-0".to_string(), true, Some("candidate-fixed"));

        assert_eq!(
            options.sender_identity.as_ref().map(ToString::to_string),
            Some("candidate-fixed".to_string())
        );
    }

    #[test]
    fn transcript_text_trims_and_drops_empty_events() {
        assert_eq!(transcript_text("  hello  "), Some("hello"));
        assert_eq!(transcript_text("  \n\t  "), None);
    }

    #[test]
    fn agent_state_attributes_preserve_existing_values() {
        let attributes = agent_state_attributes(
            HashMap::from([("role".to_string(), "interviewer".to_string())]),
            AGENT_STATE_SPEAKING,
        );

        assert_eq!(attributes.get("role"), Some(&"interviewer".to_string()));
        assert_eq!(
            attributes.get(LIVEKIT_AGENT_STATE),
            Some(&AGENT_STATE_SPEAKING.to_string())
        );
    }

    #[test]
    fn pcm16_bytes_serializes_little_endian_samples() {
        let frame = AudioFrame {
            data: [1_i16, -2_i16, 0x1234_i16].as_slice().into(),
            sample_rate: GEMINI_AUDIO_SAMPLE_RATE as u32,
            num_channels: GEMINI_AUDIO_CHANNELS as u32,
            samples_per_channel: 3,
        };

        let mut bytes = Vec::new();
        append_pcm16_bytes(&frame, &mut bytes);

        assert_eq!(bytes, vec![1, 0, 254, 255, 0x34, 0x12]);
    }

    #[test]
    fn audio_sample_rate_reads_pcm_mime_rate() {
        assert_eq!(
            audio_sample_rate("audio/pcm;rate=24000", 16_000),
            Some(24_000)
        );
        assert_eq!(audio_sample_rate("audio/pcm", 24_000), Some(24_000));
        assert_eq!(audio_sample_rate("audio/webm", 24_000), None);
    }

    /// The closing message is the one turn barge-in must not touch. Cutting it
    /// leaves the candidate without the ending, and `wrap_up_settled` reads the
    /// emptied queue as the turn being over, so the interview ended there.
    #[test]
    fn the_closing_message_is_not_cut_short_by_a_candidate_talking_over_it() {
        let (mut output_audio, _frames) = test_output_audio(Vec::new());
        let closing = output_audio.output_cancellation.clone();
        output_audio.playout_deadline = Instant::now() + Duration::from_secs(10);
        let mut activity = RuntimeActivity::new(Instant::now());
        activity.floor = Floor::AwaitingPlayout;

        assert!(take_stale_playout(&mut activity, &mut output_audio, Interruptible::No).is_none());

        assert!(!closing.is_cancelled(), "the ending has to play out");
        assert!(output_audio.is_playing());
        assert_eq!(activity.floor, Floor::AwaitingPlayout);
    }

    /// Dropping ahead of a chunk `capture` will refuse leaves the interviewer
    /// cut off mid-sentence with nothing behind it, which is worse than the
    /// queueing this whole mechanism exists to avoid.
    #[test]
    fn only_a_chunk_that_will_be_queued_is_worth_dropping_a_turn_for() {
        let (output_audio, _frames) = test_output_audio(Vec::new());
        let rate = GEMINI_OUTPUT_AUDIO_SAMPLE_RATE;

        assert!(output_audio.accepts(&[1, 0], &format!("audio/pcm;rate={rate}")));
        assert!(
            !output_audio.accepts(&[], &format!("audio/pcm;rate={rate}")),
            "an empty chunk queues nothing"
        );
        assert!(
            !output_audio.accepts(&[1, 0], "audio/pcm;rate=8000"),
            "a rate this source cannot play queues nothing"
        );
        assert!(
            !output_audio.accepts(&[1, 0], "audio/webm"),
            "an unreadable mime type queues nothing"
        );
    }

    /// The reply-latency line used to measure from `last_user_speech`, which is
    /// seeded at the interview start and only ever moved by an input
    /// transcript. A reply that followed an `Interrupted` therefore reported
    /// the age of the interview: one run logged 15.30s for a candidate who had
    /// waited under a second. The stamp is now absent until Gemini says what
    /// the candidate said, and absent means print nothing.
    /// The three rules that decide whether a reply-latency line is a
    /// measurement or a fabrication, exercised where they live rather than
    /// through a Room. Every one of them survived mutation before this test
    /// existed, which is how the log came to report 15.30s for a candidate who
    /// had waited under a second.
    #[test]
    fn the_reply_latency_stamp_is_armed_and_cleared_by_the_floor() {
        let start = Instant::now() - Duration::from_secs(15);
        let mut activity = RuntimeActivity::new(start);
        assert_eq!(
            activity.awaiting_reply_since, None,
            "nothing waited for yet"
        );

        // Armed when the candidate finishes and the agent is not talking.
        activity.floor = Floor::Listening;
        activity.note_candidate_finished(Instant::now());
        let armed = activity.awaiting_reply_since.expect("a wait to measure");
        assert!(
            armed.elapsed() < Duration::from_secs(1),
            "measured from now, not from the seed"
        );

        // Not re-armed by a late fragment that lands mid-reply: that would
        // restart the clock on a turn the candidate is already hearing.
        activity.floor = Floor::Speaking;
        activity.note_candidate_finished(Instant::now() + Duration::from_secs(5));
        assert_eq!(
            activity.awaiting_reply_since,
            Some(armed),
            "a mid-turn fragment must not re-arm"
        );

        // The idle timers still move, because that is a different question.
        assert!(
            activity.last_user_speech > armed,
            "last_user_speech tracks every fragment"
        );

        // A turn that gets cut takes the pending measurement with it: the
        // candidate is talking again, so nobody is waiting.
        let (mut output_audio, _frames) = test_output_audio(Vec::new());
        output_audio.playout_deadline = Instant::now() + Duration::from_secs(3);
        cut_off_turn(&mut activity, &mut output_audio);
        assert_eq!(
            activity.awaiting_reply_since, None,
            "a cut turn leaves nothing to measure"
        );
    }

    #[test]
    fn a_reply_nobody_was_measured_waiting_for_reports_no_latency() {
        let start = Instant::now() - Duration::from_secs(15);
        let mut activity = RuntimeActivity::new(start);

        // Nothing heard from the candidate yet, which is the state an
        // interruption leaves behind.
        assert_eq!(activity.awaiting_reply_since, None);
        assert!(
            activity.last_user_speech <= start,
            "the seed is still the interview start, and is still what the nudge reads"
        );

        // An input transcript is the only thing that starts the clock.
        activity.awaiting_reply_since = Some(Instant::now());
        let waited = activity.awaiting_reply_since.expect("stamped");
        assert!(
            waited.elapsed() < Duration::from_secs(1),
            "measured from the candidate finishing, not from the interview starting"
        );
    }

    /// `Interrupted` used to assign the floor bare. `last_agent_speech` is
    /// parked at the playout deadline while audio is queued, so leaving it
    /// there after discarding the queue suppressed the silence nudge for the
    /// length of speech nobody heard.
    #[test]
    fn a_cut_off_turn_stamps_the_moment_it_was_cut_not_when_it_would_have_ended() {
        let (mut output_audio, _frames) = test_output_audio(Vec::new());
        let cut = output_audio.output_cancellation.clone();
        output_audio.playout_deadline = Instant::now() + Duration::from_secs(15);
        let mut activity = RuntimeActivity::new(Instant::now());
        activity.last_agent_speech = output_audio.playout_deadline;
        activity.floor = Floor::Speaking;

        let unplayed = cut_off_turn(&mut activity, &mut output_audio);

        assert!(unplayed >= Duration::from_secs(14), "reports {unplayed:?}");
        assert!(cut.is_cancelled(), "queued frames must be dropped");
        assert_eq!(activity.floor, Floor::Listening);
        assert!(
            activity.last_agent_speech <= Instant::now(),
            "the stamp must not sit in the future, where it suppresses the nudge"
        );
    }

    /// The first candidate answer used to land behind the rest of the greeting.
    /// Gemini streams a twenty second greeting in about two, marks the turn
    /// complete, and then never reports an interruption, because from its side
    /// that turn is long over. Only this process knows the queue is still
    /// draining.
    #[test]
    fn a_candidate_speaking_over_a_draining_turn_drops_what_is_left_of_it() {
        let (mut output_audio, _frames) = test_output_audio(Vec::new());
        let stale = output_audio.output_cancellation.clone();
        output_audio.playout_deadline = Instant::now() + Duration::from_secs(10);
        let mut activity = RuntimeActivity::new(Instant::now());
        activity.floor = Floor::AwaitingPlayout;

        let dropped = take_stale_playout(&mut activity, &mut output_audio, Interruptible::Yes)
            .expect("a draining turn must be dropped");
        assert!(
            dropped >= Duration::from_secs(9),
            "reports what it dropped: {dropped:?}"
        );

        assert!(
            stale.is_cancelled(),
            "queued greeting frames must be dropped"
        );
        assert!(!output_audio.is_playing(), "the deadline must come forward");
        assert_eq!(activity.floor, Floor::Listening);
    }

    /// The guard is two conditions and both are load bearing. `AwaitingPlayout`
    /// is stamped once when the turn completes and outlives the queue it was
    /// named for, so on its own it would cut into a turn that is only just
    /// starting.
    #[test]
    fn nothing_is_dropped_once_the_queue_has_drained_or_while_the_agent_speaks() {
        for (floor, deadline, why) in [
            (
                Floor::AwaitingPlayout,
                Instant::now(),
                "queue already drained",
            ),
            (
                Floor::Speaking,
                Instant::now() + Duration::from_secs(10),
                "turn still being produced",
            ),
            (
                Floor::Listening,
                Instant::now() + Duration::from_secs(10),
                "candidate already holds the floor",
            ),
        ] {
            let (mut output_audio, _frames) = test_output_audio(Vec::new());
            let live = output_audio.output_cancellation.clone();
            output_audio.playout_deadline = deadline;
            let mut activity = RuntimeActivity::new(Instant::now());
            activity.floor = floor;

            assert!(
                take_stale_playout(&mut activity, &mut output_audio, Interruptible::Yes).is_none(),
                "{why}"
            );
            assert!(!live.is_cancelled(), "{why}");
            assert_eq!(activity.floor, floor, "{why}");
        }
    }

    fn test_output_audio(
        pending_bytes: Vec<u8>,
    ) -> (OutputAudio, mpsc::Receiver<QueuedOutputFrame>) {
        let (frames, queued_frames) = mpsc::channel(4);
        (
            OutputAudio {
                source: NativeAudioSource::new(
                    AudioSourceOptions::default(),
                    GEMINI_OUTPUT_AUDIO_SAMPLE_RATE,
                    LIVEKIT_OUTPUT_CHANNELS,
                    LIVEKIT_OUTPUT_QUEUE_MS,
                ),
                sample_rate: GEMINI_OUTPUT_AUDIO_SAMPLE_RATE,
                pending_bytes,
                playout_deadline: Instant::now(),
                frames,
                output_cancellation: CancellationToken::new(),
            },
            queued_frames,
        )
    }

    #[tokio::test]
    async fn output_audio_capture_queues_frames_with_current_cancellation_token() {
        let (mut output_audio, mut queued_frames) = test_output_audio(Vec::new());
        let start_deadline = output_audio.playout_deadline;
        let bytes = vec![0_u8; 240 * 2];

        assert!(
            output_audio
                .capture(&bytes, "audio/pcm;rate=24000")
                .await
                .unwrap()
        );

        let frame = queued_frames.try_recv().unwrap();
        assert!(!frame.output_cancellation.is_cancelled());
        assert_eq!(frame.samples.len(), 240);
        assert!(output_audio.playout_deadline > start_deadline);
        output_audio.interrupt();
        assert!(frame.output_cancellation.is_cancelled());
        assert!(queued_frames.try_recv().is_err());
    }

    #[test]
    fn output_audio_interrupt_clears_partial_pcm_frame_and_cancels_old_queue() {
        let (mut output_audio, _) = test_output_audio(vec![1, 2, 3]);
        let old_cancellation = output_audio.output_cancellation.clone();
        output_audio.playout_deadline = Instant::now() + Duration::from_secs(10);

        output_audio.interrupt();

        assert!(output_audio.pending_bytes.is_empty());
        assert!(old_cancellation.is_cancelled());
        assert!(!output_audio.output_cancellation.is_cancelled());
        assert!(!output_audio.is_playing());
    }

    #[test]
    fn take_pcm16_frames_decodes_full_ten_ms_frames_and_keeps_remainder() {
        let frame_bytes = 240 * 2;
        let mut bytes = vec![0_u8; frame_bytes + 2];
        bytes[0] = 1;
        bytes[1] = 0;
        bytes[frame_bytes] = 9;
        let mut pending = Vec::new();

        let frames = take_pcm16_frames(&bytes, 24_000, 1, &mut pending);

        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].len(), 240);
        assert_eq!(frames[0][0], 1);
        assert_eq!(pending, vec![9, 0]);
    }

    #[test]
    fn video_frame_throttle_matches_gemini_live_limit() {
        assert!(!should_send_video_frame(Duration::from_millis(999)));
        assert!(should_send_video_frame(Duration::from_secs(1)));
    }

    #[test]
    fn encode_video_frame_jpeg_outputs_jpeg_bytes() {
        let buffer: Box<dyn VideoBuffer> = Box::new(I420Buffer::new_black(16, 16));
        let frame = VideoFrame {
            rotation: VideoRotation::VideoRotation0,
            timestamp_us: 0,
            frame_metadata: None,
            buffer,
        };

        let bytes = encode_video_frame_jpeg(&frame, GEMINI_VIDEO_JPEG_QUALITY).unwrap();

        assert!(bytes.starts_with(&[0xff, 0xd8]));
        assert!(bytes.ends_with(&[0xff, 0xd9]));
    }

    /// A black frame cannot catch a permuted channel, because every channel
    /// holds the same value. This one is solid red in BT.601 limited range, so
    /// the four libyuv layouts land on four different byte patterns.
    #[test]
    fn channel_order_survives_the_libyuv_naming_trap() {
        let mut buffer = I420Buffer::new(16, 16);
        let (luma, blue, red) = buffer.data_mut();
        luma.fill(81);
        blue.fill(90);
        red.fill(240);
        let frame = VideoFrame {
            rotation: VideoRotation::VideoRotation0,
            timestamp_us: 0,
            frame_metadata: None,
            buffer: Box::new(buffer) as Box<dyn VideoBuffer>,
        };

        let (rgba, width, height) = frame_to_rgba(&frame).unwrap();

        assert_eq!((width, height), (16, 16));
        let pixel = &rgba[..4];
        assert!(pixel[0] > 200, "red belongs in byte 0, got {pixel:?}");
        assert!(pixel[1] < 60, "green belongs in byte 1, got {pixel:?}");
        assert!(pixel[2] < 60, "blue belongs in byte 2, got {pixel:?}");
        assert_eq!(pixel[3], 255, "alpha belongs in byte 3, got {pixel:?}");
    }
}
#[test]
fn observer_is_not_the_candidate() {
    assert!(candidate_identity_matches(
        "candidate-a1b2c3",
        r#"{"candidateIdentity":"candidate-a1b2c3"}"#,
    ));
    assert!(!candidate_identity_matches("observer-a1b2c3", "{}"));
    assert!(!candidate_identity_matches(
        "candidate-a1b2c3",
        r#"{"candidateIdentity":"candidate-other"}"#,
    ));
}
