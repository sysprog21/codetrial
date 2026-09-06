//! The agent's side of a room: join, stream the candidate's audio and video to
//! Gemini, publish Gemini's audio back, and produce the report when the
//! interview ends.
//!
//! Still the largest module in the tree, so here is its shape.
//!
//! Four regions are out. `media` holds the buffers, pumps and codecs that carry
//! audio and video in both directions, and `turn` holds the state a turn is
//! made of, which explains what stayed behind with the loop. `rooms` is the
//! only region that talks to LiveKit over HTTP rather than over the room's
//! event stream, and `report` is the packet an ending interview produces.
//!
//! What is left is the loop and the things it is made of:
//!
//! - `run_room` and `join_room`: the lifecycle, and the only entry point.
//! - Session setup and restart (`take_restart_attempt` through `open_session`):
//!   what runs once before the loop, and what the loop calls when the Gemini
//!   socket dies under it.
//! - Event handling (`handle_data_packet`, `handle_gemini_event`): the sources
//!   that drive the session. The third, `handle_media_event`, is in `media`.
//! - Turn procedures (`send_wrap_up_and_wait` through `close_turn`): what ends
//!   a turn, operating on the context above.
//!
//! Session setup is the region that looks separable and is not, which is worth
//! writing down so the next reader does not spend the afternoon finding out.
//! `open_session` and `restart_after_close` name `InterviewContext`,
//! `GeminiEventContext`, `OutputAudio`, `CandidateMedia` and `TurnState`, and
//! call `join_room`, `close_turns`, `cut_off_turn`, `set_agent_state`,
//! `publish_interviewer_state` and `candidate_bootstrap`. Moving it out moves
//! the loop's own vocabulary with it, and what is gained is a file boundary
//! rather than a seam. The three that did come out each named four parent items
//! or fewer.

use std::collections::HashMap;
use std::ops::ControlFlow;
use std::time::{Duration, Instant};

use ::livekit::DisconnectReason;
use ::livekit::ParticipantKind;
use ::livekit::data_stream::api::StreamTextOptions;
use ::livekit::prelude::{DataPacket, RemoteParticipant, Room, RoomEvent, RoomOptions};

use crate::agent::{
    RuntimeState, SpeakerTurn, WATCH_TICK_S, apply_data_event, framework_evidence_json,
    framework_progress, parse_participant_metadata, read_editor_text, record_framework_evidence,
    wrap_up,
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
    GeminiEvent, GeminiFunctionCall, GeminiLiveSession, open_live_session, resume_live_session,
};
use crate::runtime::{
    AGENT_NAME, RuntimeBootstrap, TOOL_LOG_HINT, TOOL_READ_EDITOR, TOOL_RECORD_FRAMEWORK_EVIDENCE,
    TOPIC_TRANSCRIPTION, agent_identity,
};
use crate::token::{LivekitTokenInput, livekit_token};

mod media;
mod report;
mod rooms;
mod turn;

use report::publish_report;
use rooms::{evict_duplicate_agent, isolate_local_agent};

// Re-exported rather than merely used: `web::setup` calls this to check the
// credentials a Setup form was given, and it names it through this module.
pub(crate) use rooms::validate_livekit_credentials;

use media::*;
use turn::*;

const GEMINI_OUTPUT_AUDIO_SAMPLE_RATE: u32 = 24_000;
/// How many times in a row the interview may be put back on a fresh Gemini
/// socket after a close that arrived too quickly to be Gemini's ordinary
/// connection cap. In a row, because `take_restart_attempt` clears the count
/// for any socket that lived past `HEALTHY_GEMINI_SOCKET`: this bounds a
/// failing endpoint, not the length of an interview.
const GEMINI_RESTART_LIMIT: usize = 8;

/// Past this, a closed socket was a working one, whatever closed it. Gemini
/// caps a connection at around ten minutes and a healthy interview crosses that
/// cap several times, so the restart budget must not be spent on the crossings.
const HEALTHY_GEMINI_SOCKET: Duration = Duration::from_secs(60);

/// Waited between cold opens that did not reach Gemini at all.
///
/// Without it the retry is not a retry. A refused connection, a DNS miss and a
/// 503 all come back in milliseconds, so the whole budget is spent inside one
/// second, every attempt asks the same unavailable endpoint the same question,
/// and the interview ends on an outage that would have cleared while the
/// candidate was still mid-sentence. Two seconds spreads the budget over the
/// dozen seconds such an outage actually lasts, and it costs nothing on the
/// timeout path, where the attempt already took fifteen.
const COLD_OPEN_BACKOFF: Duration = Duration::from_secs(2);
const LIVEKIT_AGENT_STATE: &str = "lk.agent.state";
const AGENT_STATE_LISTENING: &str = "listening";
const AGENT_STATE_SPEAKING: &str = "speaking";
const DUPLICATE_AGENT_ISOLATION_ATTEMPTS: usize = 20;
const WRAP_UP_WAIT: Duration = Duration::from_secs(8);

/// Below this, a queued turn is not a wait anyone experiences, and saying so
/// costs a log line per turn that reads as zero.
const NOTABLE_PLAYOUT_BACKLOG: Duration = Duration::from_millis(500);
/// Covers the normal report attempt, one schema repair, and bounded transient
/// retries. The candidate is watching a spinner, so this is the point where
/// waiting stops being worth more than an honest incomplete report.
pub(super) const REPORT_TIMEOUT: Duration = Duration::from_secs(45);
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

/// Decides whether the interview may be put back on a fresh Gemini socket, and
/// spends one of the budgeted attempts when it may.
///
/// This is the half of restarting that can be judged. `cargo test` cannot open
/// a Gemini session, but every rule about when not to try one is arithmetic
/// over a counter and a clock, so it lives here where tests can reach it rather
/// than inside the reconnect it authorises.
///
/// The budget counts restarts, not resumptions, and says nothing about whether
/// a handle exists. Missing one costs the conversation so far, not the
/// interview, so it is the caller's choice between two kinds of restart rather
/// than a reason to stop.
///
/// The counter resets for a socket that lived a while, because the ceiling is
/// there to stop a socket that fails instantly from spinning, and a socket that
/// carried seven minutes of interview did not fail instantly. Counting those
/// for the life of the session made the ceiling a limit on interview length: at
/// Gemini's own connection cap a ninety-minute interview spends most of the
/// budget before anything goes wrong, and whatever is left is what a network
/// blip gets to consume.
fn take_restart_attempt(restarts: &mut usize, socket_age: Duration) -> bool {
    if socket_age >= HEALTHY_GEMINI_SOCKET {
        *restarts = 0;
    }
    if *restarts >= GEMINI_RESTART_LIMIT {
        return false;
    }

    // Spent before the reconnect rather than after it. A socket that fails to
    // come back is the case the ceiling exists for, and counting only the
    // successes would let an endpoint failing instantly be retried without
    // bound.
    *restarts += 1;
    true
}

/// Opens a session that remembers nothing, retrying while the budget allows.
///
/// `None` is the budget running out, which is the caller's cue to end the
/// interview. Split from `restart_after_close` because a loop with its own exit
/// inside an arm of a match inside an arm of a match put the give-up path four
/// levels deep in a function that is otherwise a sequence of steps.
async fn open_cold_session(
    interview: InterviewContext<'_>,
    restarts: &mut usize,
) -> Option<GeminiLiveSession> {
    loop {
        match open_live_session(&interview.config.google_api_key, interview.boot).await {
            Ok(session) => return Some(session),
            Err(error) if take_restart_attempt(restarts, Duration::ZERO) => {
                eprintln!("Gemini could not be reached ({error}); retrying cold session");
                tokio::time::sleep(COLD_OPEN_BACKOFF).await;
            }
            Err(error) => {
                eprintln!("Gemini could not be reached ({error}); the budget is spent");
                return None;
            }
        }
    }
}

/// Puts the interview back on a new Gemini socket after the old one closed, or
/// reports that it cannot be.
///
/// `Break` means the interview is over. Nothing restarts it: the dispatcher
/// spawns one task per room and drops the slot when it returns, so this leaves
/// the candidate in a live room with no interviewer, which is what the browser
/// says when it sees the agent go.
///
/// That is why a missing or rejected resumption handle is not the end. Resuming
/// keeps what was said; a cold session keeps only what the system instruction
/// carries, which is the problem, the rubric and the round, plus the editor
/// snapshot handed over below. Losing the conversation is a worse interview.
/// Losing the interviewer is no interview.
///
/// The reason the socket closed came over it and is logged by the reader task.
/// The lines below tie that to the room.
async fn restart_after_close(
    room: &Room,
    context: &mut GeminiEventContext<'_>,
    interview: InterviewContext<'_>,
    restarts: &mut usize,
) -> Result<ControlFlow<()>, Box<dyn std::error::Error + Send + Sync>> {
    if !take_restart_attempt(restarts, context.gemini.age()) {
        eprintln!(
            "Gemini closed {restarts} sockets in a row without one of them lasting; ending interview room={}",
            interview.boot.room_name
        );
        leave_room(room).await;
        return Ok(ControlFlow::Break(()));
    }

    // Said before the attempt, not after it: the whole point is to cover the
    // gap, and the gap starts here. Connect and setup are bounded at fifteen
    // seconds each, and for that long the candidate is talking to a socket that
    // is gone.
    publish_interviewer_state(room, true).await?;

    // The socket is already gone; this closes the writer half and the reader
    // task. A close on a dead socket errors, and that error says nothing the
    // caller can act on.
    let _ = context.gemini.shutdown().await;

    // A handle is worth one try and no more: one the server refuses fails
    // identically every time, so retrying it under the budget would spend the
    // whole budget learning the same thing. Both ways of not having a resumable
    // session -- no handle, and a handle that was refused -- leave a `None` for
    // the cold start below to answer, which is why neither is a branch of its
    // own here.
    let resumed_session = match context.gemini.resumption_handle() {
        Some(handle) => {
            resume_live_session(&interview.config.google_api_key, interview.boot, &handle)
                .await
                .inspect_err(|error| {
                    eprintln!(
                        "Gemini refused to resume ({error}); starting a fresh session instead"
                    );
                })
                .ok()
        }
        None => None,
    };

    let resumed = resumed_session.is_some();
    let session = match resumed_session {
        Some(session) => Some(session),
        None => open_cold_session(interview, restarts).await,
    };
    let Some(session) = session else {
        eprintln!(
            "Gemini could not be reached; ending interview room={}",
            interview.boot.room_name
        );
        leave_room(room).await;
        return Ok(ControlFlow::Break(()));
    };

    *context.gemini = session;

    // Whatever was mid-flight died with the socket. The turn ids have to close
    // here for the same reason an interruption closes them: left open, the next
    // thing either party says appends to an utterance that was cut off, and the
    // panel and the report both read the two as one.
    cut_off_turn(context.activity, context.output_audio);

    // The discard belonged to the socket that just died. It is set when a pause
    // cuts a reply in flight and cleared by the `turnComplete` or `interrupted`
    // that answers it, which a closed socket never sends, so a restart in that
    // window left it set and the new session's first turn was dropped on the
    // way out. That turn is the cold-restart briefing, which is the one turn
    // this whole path exists to deliver.
    context.activity.discarding_output = false;
    close_turns(room, context).await?;
    set_agent_state(room, context.agent_state, AGENT_STATE_LISTENING).await?;
    publish_interviewer_state(room, false).await?;

    if resumed {
        eprintln!("Gemini session resumed; the interview continues where it left off");
        return Ok(ControlFlow::Continue(()));
    }

    // A cold session has never heard this candidate. Without this it waits for
    // someone to speak first, holding whatever they say against a rubric it
    // thinks nobody has started yet, and the editor is the one part of the lost
    // conversation that still exists in this process.
    eprintln!("Gemini session restarted cold; the interview continues without what was said");

    // Except while the interview is paused, which is the one state where making
    // Jim talk is the wrong move: the reply would be discarded on the way out.
    // The briefing is owed rather than skipped, and unpausing is what pays it,
    // because that is the next moment Jim speaks at all.
    if context.state.paused {
        context.state.needs_cold_brief = true;
        return Ok(ControlFlow::Continue(()));
    }

    // Not `?`, for the reason the watch loop already gives about writes to
    // Gemini: a socket that came up and died again reports the close through
    // `next_event`, where this arm is waiting to restart it. Propagating here
    // would end the interview on the one write the restart exists to make, and
    // the write most likely to meet a socket that is already gone.
    if let Err(error) = context
        .gemini
        .send_text(&crate::agent::cold_restart(context.state))
        .await
    {
        eprintln!("cold-restart briefing failed ({error}); waiting for the close to be reported");
        return Ok(ControlFlow::Continue(()));
    }
    context.activity.mark_speaking();
    Ok(ControlFlow::Continue(()))
}

/// Everything the interview loop needs, owned, once the candidate has joined
/// and both sides are talking.
///
/// Owned and not a set of borrows: the loop builds `InterviewContext` and
/// `RoomIdentities` out of these fields, and a struct that held those views
/// alongside the values they point at would be borrowing itself. `'a` is the
/// caller's, carried only by `boot`.
struct OpenSession<'a> {
    room: Room,
    events: tokio::sync::mpsc::UnboundedReceiver<RoomEvent>,
    agent_identity: String,
    candidate_identity: String,
    boot: RuntimeBootstrap<'a>,
    output_audio: OutputAudio,
    gemini: crate::gemini::GeminiLiveSession,
    media: CandidateMedia,
    turn: TurnState,
    started_at: Instant,
}

/// Joins, waits for a candidate, brings up audio and Gemini, and greets.
///
/// Split from [`run_room`] because none of it repeats: it runs once, in order,
/// and every line of it is a step that either succeeds or ends the interview
/// before it starts. What is left in `run_room` is the part that runs many
/// times, which is the part worth reading as a loop.
///
/// `Ok(None)` is nobody joined. That is not an error: the room was named, the
/// candidate never arrived, and there is nothing to run or report.
async fn open_session<'a>(
    config: &'a AgentConfig,
    room_name: &'a str,
    now_seconds: u64,
) -> Result<Option<OpenSession<'a>>, Box<dyn std::error::Error + Send + Sync>> {
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
        return Ok(None);
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
    let (output_audio, mut gemini) = tokio::try_join!(
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

    // The candidate's clock, not a third one. The browser starts counting when
    // its own connect resolves, which is this instant seen from the other side,
    // and stamping again here put the agent a second or two behind the tab for
    // the rest of the interview. Every consumer below reads this as "when the
    // interview started", and that is when it started.
    let started_at = setup_began;

    let mut turn = TurnState {
        state: initial_runtime_state(&boot, started_at),
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

    Ok(Some(OpenSession {
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
    }))
}

pub async fn run_room(
    config: &AgentConfig,
    room_name: &str,
    now_seconds: u64,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let Some(OpenSession {
        room,
        mut events,
        agent_identity,
        candidate_identity,
        boot,
        mut output_audio,
        mut gemini,
        mut media,
        mut turn,
        started_at,
    }) = open_session(config, room_name, now_seconds).await?
    else {
        return Ok(());
    };

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

    let mut restarts = 0usize;

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
    let hard_deadline = tokio::time::sleep_until(
        (Instant::now()
            + Duration::from_secs(u64::from(boot.duration_min) * 60)
            + INTERVIEW_DEADLINE_GRACE)
            .into(),
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
                    leave_room(&room).await;
                    return Ok(());
                }
                if let Some(prompt) = turn.activity.watch_prompt(&turn.state, Instant::now()) {
                    // Not `?`. Every write below is one the reader may be about
                    // to explain: a socket Gemini has closed fails the next
                    // send long before `next_event` drains and reports it, and
                    // audio flushes every hundred milliseconds, so propagating
                    // here is how a closed socket ended the interview from the
                    // write side without the arm that resumes it ever running.
                    if let Err(error) = gemini.send_text(&prompt).await {
                        eprintln!("Gemini nudge failed ({error}); waiting for the close to be reported");
                        continue;
                    }
                    turn.activity.mark_speaking();
                }
            }
            event = events.recv() => {
                let Some(event) = event else {
                    eprintln!(
                        "LiveKit event stream ended for room={room_name}; ending with no report, \
                         because the room closed before the interview did"
                    );
                    gemini.close().await?;
                    return Ok(());
                };
                match handle_media_event(
                    &mut media,
                    &mut gemini,
                    &candidate_identity,
                    config.gemini_candidate_video_enabled,
                    &event,
                )
                .await
                {
                    Ok(true) => continue,
                    Ok(false) => {}
                    Err(error) => {
                        eprintln!("Gemini media attach failed ({error}); waiting for the close to be reported");
                        continue;
                    }
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
                    // Gemini hung up. Usually that is the ten-minute cap on a
                    // single connection rather than anything wrong, so the
                    // session continues on a new socket instead of ending the
                    // interview.
                    let mut context = turn.context(&mut output_audio, &mut gemini, media.identity.as_deref());
                    if restart_after_close(&room, &mut context, interview, &mut restarts)
                        .await?
                        .is_break()
                    {
                        return Ok(());
                    }
                    continue;
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
                // The fastest writer in the loop, and so the one that reaches a
                // closed socket first: a flush leaves every hundred
                // milliseconds of speech. Dropping the frame costs a tenth of a
                // second of audio the resumed session did not need; propagating
                // cost the interview.
                let ended = frame.is_none();
                if turn.state.paused {
                    discard_paused_audio(&mut media);
                } else if let Err(error) = pump_audio(&mut media, &mut gemini, frame).await {
                    eprintln!("Gemini audio write failed ({error}); waiting for the close to be reported");
                }

                // One release for the arm, past every branch above it. Sitting
                // inside a branch is what let a failed final flush keep an
                // ended stream, and there is no path through here that wants to
                // hold on to one.
                release_if_ended(&mut media.audio, ended);
            }
            frame = next_video_frame(&mut media.video), if media.video.is_some() => {
                if turn.state.paused {
                    release_if_ended(&mut media.video, frame.is_none());
                } else if let Err(error) = pump_video(&mut media, &mut gemini, frame).await {
                    eprintln!("Gemini video write failed ({error}); waiting for the close to be reported");
                }
            }
        }
    }
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
                // The browser asking to end is the ordinary way an interview
                // finishes, and it was the one exit that said nothing.
                // Indistinguishable in the log from the agent dying and being
                // restarted under it.
                eprintln!("interview ended by the browser: room={room_name} topic={topic}");
                return Ok(ControlFlow::Break(()));
            }
        }

        // The server's own word for why the agent is going away. Without it the
        // only trace is the event channel closing a moment later, which says a
        // disconnect happened and nothing about whose fault it was: a duplicate
        // identity, a deleted room and a signal drop all look identical from
        // there, and they need three different fixes.
        //
        // `DuplicateIdentity` is the one worth naming outright. The agent
        // identity is derived from the room name, so a second agent process on
        // the same room is not a near-miss, it is the same string, and the
        // server evicts whichever joined first. `is_duplicate_agent` cannot see
        // that case: it matches on the identity being different.
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
            evict_duplicate_agent(config, room_name, &participant.identity().0, now_seconds)
                .await?;
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

/// Disconnects this agent, which is all `Room::close` does: the room and the
/// candidate in it outlive this by whatever LiveKit's own timeouts say. Named
/// for what happens rather than for what the call is called, because every
/// caller is deciding whether to abandon an interview, and "close the room"
/// reads like the room goes with it.
async fn leave_room(room: &Room) {
    if let Err(error) = room.close().await {
        eprintln!("leaving the room failed, ignoring: {error}");
    }
}

/// Whether the interviewer can hear the candidate right now.
///
/// The agent stays in the room across a Gemini restart, so nothing the browser
/// watches changes: the participant is there, the audio track is published, and
/// the candidate talks into a socket that is being rebuilt. Up to thirty
/// seconds of that is possible, which is long enough that saying nothing reads
/// as the interviewer ignoring them.
async fn publish_interviewer_state(
    room: &Room,
    reconnecting: bool,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    room.local_participant()
        .publish_data(browser_packet(
            TOPIC_CONTROL,
            &serde_json::json!({ "type": "interviewer_state", "reconnecting": reconnecting }),
        )?)
        .await?;
    Ok(())
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
    crate::runtime::bootstrap_with_rounds(
        config,
        room_name,
        Some(candidate.problem.id),
        candidate.duration_min,
        crate::runtime::RuntimeOptions {
            profile: candidate.profile,
            grounding: candidate.grounding,
            interview_loop: candidate.interview_loop,
        },
    )
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

/// The interview's starting state, from the plan the token was minted for.
///
/// Every field here is read for the length of the interview and none can be
/// recovered once it is missed: the clock the round boundary is measured
/// against, the loop that decides whether a behavioral round exists at all,
/// and the two budgets the report divides the session into.
fn initial_runtime_state(boot: &RuntimeBootstrap<'_>, started_at: Instant) -> RuntimeState {
    RuntimeState {
        started_at,
        interview_loop: boot.interview_loop,
        coding_minutes: boot.coding_minutes,
        behavioral_minutes: boot.behavioral_minutes,
        ..RuntimeState::default()
    }
}

/// A packet for the browser, on the topic it is listening to.
///
/// The three fields here are what decide whether the message arrives at all:
/// an unreliable one may be dropped on a bad network, and one with no topic
/// lands on a channel nothing reads. Built in one place because it was written
/// out at four call sites, where each field was free to go missing on its own.
pub(super) fn browser_packet(
    topic: &str,
    message: &serde_json::Value,
) -> Result<DataPacket, serde_json::Error> {
    Ok(DataPacket {
        payload: serde_json::to_vec(message)?,
        topic: Some(topic.to_string()),
        reliable: true,
        ..Default::default()
    })
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
    if let Some(paused) = result.pause_changed {
        room.local_participant()
            .publish_data(browser_packet(
                TOPIC_CONTROL,
                &serde_json::json!({ "type": "pause_state", "paused": paused }),
            )?)
            .await?;
        if paused && context.activity.floor != Floor::Listening {
            context.activity.discarding_output =
                pause_leaves_output_in_flight(context.activity.floor);
            cut_off_turn(context.activity, context.output_audio);
            close_turns(room, context).await?;
            set_agent_state(room, context.agent_state, AGENT_STATE_LISTENING).await?;
        }
    }
    if let Some(status) = result.round_changed {
        room.local_participant()
            .publish_data(browser_packet(
                TOPIC_CONTROL,
                &serde_json::json!({
                    "type": "round_state", "round": "behavioral", "status": status
                }),
            )?)
            .await?;
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
    // Give the report packet a moment to leave before the agent goes.
    tokio::time::sleep(Duration::from_millis(250)).await;
    leave_room(room).await;
    Ok(ControlFlow::Break(()))
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

async fn handle_gemini_event(
    room: &Room,
    context: &mut GeminiEventContext<'_>,
    event: GeminiEvent,
    interruptible: Interruptible,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if context.activity.discarding_output {
        match &event {
            GeminiEvent::Audio { .. } | GeminiEvent::OutputTranscript(_) => return Ok(()),
            GeminiEvent::TurnComplete | GeminiEvent::Interrupted => {
                context.activity.discarding_output = false;
            }
            _ => {}
        }
    }
    if context.state.paused
        && matches!(
            &event,
            GeminiEvent::Audio { .. } | GeminiEvent::OutputTranscript(_)
        )
    {
        return Ok(());
    }
    match event {
        GeminiEvent::ToolCall(calls) => {
            for call in calls {
                let shown_before = framework_progress(context.state);
                let response = execute_tool_call(context.state, &call);
                context.gemini.send_tool_response(&call, response).await?;
                if checklist_changed(&shown_before, context.state) {
                    publish_framework_progress(room, context.state).await?;
                }
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

            // Only a backlog a candidate would notice. `!is_zero()` fired on a
            // millisecond and printed "0.0s", so every one of these lines in a
            // real session said nothing at all.
            if backlog >= NOTABLE_PLAYOUT_BACKLOG {
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

            // What Gemini heard is the whole diagnosis. It interrupts on its
            // own voice activity detection, so a cut with the candidate
            // mid-sentence is barge-in working, and a cut with nothing
            // transcribed is the microphone hearing the interviewer through the
            // candidate's speakers. The line reported the size of the loss and
            // left the cause to guesswork across a whole session of them.
            let heard = context.turns.candidate.tail(80);
            eprintln!(
                "timing: Gemini cut its own turn, {:.1}s of it unplayed; candidate audio so far: {}",
                unplayed.as_secs_f64(),
                if heard.is_empty() {
                    "(nothing transcribed)"
                } else {
                    heard
                }
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
            serde_json::json!({ "result": crate::agent::record_hint(state) })
        }
        TOOL_RECORD_FRAMEWORK_EVIDENCE => match record_framework_evidence(state, &call.args) {
            Ok(evidence) => serde_json::json!({ "result": framework_evidence_json(&evidence) }),
            Err(error) => serde_json::json!({ "error": error }),
        },
        name => serde_json::json!({ "error": format!("unknown tool: {name}") }),
    }
}

/// Whether the candidate's checklist would look any different now.
///
/// The tool is idempotent and returns the existing entry for a repeat, and
/// evidence for a phase they never reached is recorded but never shown. Asking
/// how much has been recorded instead would redraw the checklist with nothing
/// new in it, and reveal an empty one for a skip banked before any phase was.
fn checklist_changed(shown_before: &[&'static str], state: &RuntimeState) -> bool {
    framework_progress(state) != shown_before
}

/// What the candidate is allowed to see of their own framework progress: which
/// phases have evidence, and nothing else.
///
/// The interviewer names the step it is steering toward out loud, so a phase it
/// has already banked is not a secret. The summary, confidence and source stay
/// server-side, because those are the reading rather than the fact.
async fn publish_framework_progress(
    room: &Room,
    state: &RuntimeState,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    room.local_participant()
        .publish_data(browser_packet(
            TOPIC_CONTROL,
            &serde_json::json!({
                "type": "framework_state",
                "phases": crate::agent::framework_progress(state),
            }),
        )?)
        .await?;
    Ok(())
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

/// The room loop's event bundle, borrowed out of the turn state that owns most
/// of it. Here rather than beside `TurnState` because `GeminiEventContext` is
/// this module's type: the turn state is data the loop keeps, and this is the
/// one place the two are stitched together.
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

#[cfg(test)]
mod tests {
    use super::*;

    // Only the tests below reach for it now: the report packet that carries
    // this topic is built in `report.rs`, and what is left here asserts which
    // topics the loop refuses from a browser.
    use crate::runtime::TOPIC_REPORT;

    use crate::config::load_from_pairs;

    /// The interview starts from the plan its token was minted for.
    ///
    /// None of this can be recovered once it is missed: the clock the round
    /// boundary is measured against, the loop that decides whether there is a
    /// behavioral round at all, and the budgets the report divides the session
    /// into. A field dropped here is a default carried for the whole hour.
    #[test]
    fn the_interview_begins_from_the_plan_it_was_booked_with() {
        let config = load_from_pairs([
            ("LIVEKIT_URL", "wss://example.livekit.cloud"),
            ("LIVEKIT_API_KEY", "devkey"),
            ("LIVEKIT_API_SECRET", "devsecret"),
            ("GOOGLE_API_KEY", "google-key"),
        ])
        .unwrap();

        // A plan that differs from the default in every field, because the
        // default is the forty-five minute two-round one: booked with that, a
        // field dropped here still reads correctly and the test proves nothing.
        let boot = crate::runtime::bootstrap_with_rounds(
            &config,
            "interview-fixed",
            Some("two-sum"),
            30,
            crate::runtime::RuntimeOptions {
                interview_loop: crate::agent::InterviewLoop::CodingOnly,
                ..crate::runtime::RuntimeOptions::default()
            },
        );
        let untouched = RuntimeState::default();
        for (named, same) in [
            (
                "interview_loop",
                boot.interview_loop == untouched.interview_loop,
            ),
            (
                "coding_minutes",
                boot.coding_minutes == untouched.coding_minutes,
            ),
            (
                "behavioral_minutes",
                boot.behavioral_minutes == untouched.behavioral_minutes,
            ),
        ] {
            assert!(
                !same,
                "{named} matches the default, so dropping it is invisible"
            );
        }

        let started_at = Instant::now() - Duration::from_secs(300);
        let state = initial_runtime_state(&boot, started_at);

        assert_eq!(
            state.started_at, started_at,
            "the clock is the room's, not now"
        );
        assert_eq!(state.interview_loop, boot.interview_loop);
        assert_eq!(state.coding_minutes, boot.coding_minutes);
        assert_eq!(state.behavioral_minutes, boot.behavioral_minutes);
    }

    /// Closing a turn yields what to publish, once, and only for an open one.
    ///
    /// The pair is the line and the segment it replaces, so an empty text or a
    /// borrowed id publishes a turn that says nothing or overwrites another
    /// one. Closing a turn nobody opened would publish a blank line for a
    /// speaker who has not spoken.
    #[test]
    fn a_turn_closes_once_and_only_when_it_was_open() {
        let mut unopened = SpeakerTurn::default();
        assert!(
            close_turn(&mut unopened, "Candidate").is_none(),
            "a speaker who has not spoken has no line to publish"
        );

        let mut turn = SpeakerTurn::default();
        turn.record(&mut Vec::new(), "Candidate", "I will use a hash map.");
        let open_segment = turn.segment_id("Candidate");
        let (text, segment) = close_turn(&mut turn, "Candidate").expect("an open turn closes");
        assert_eq!(text, "I will use a hash map.");
        assert_eq!(
            segment, open_segment,
            "the id names the segment being replaced, not the one after it"
        );
        assert_ne!(
            turn.segment_id("Candidate"),
            open_segment,
            "and the next line is a new segment, not an overwrite of this one"
        );

        assert!(
            close_turn(&mut turn, "Candidate").is_none(),
            "a closed turn closes once; publishing it again repeats the line"
        );
    }

    /// A packet the browser never receives is the same as one never sent.
    ///
    /// The topic is the channel it listens on and reliability is whether a bad
    /// network may drop it, and both were written out at each call site where
    /// either could go missing on its own. The report and the control messages
    /// differ only in which topic they name.
    #[test]
    fn a_published_packet_carries_its_topic_and_arrives_reliably() {
        for topic in [TOPIC_CONTROL, TOPIC_REPORT] {
            let packet = browser_packet(topic, &serde_json::json!({"type": "pause_state"}))
                .expect("a json object serializes");
            assert_eq!(
                packet.topic.as_deref(),
                Some(topic),
                "no topic is a channel nobody is reading"
            );
            assert!(packet.reliable, "the browser only gets one of these");
            let payload: serde_json::Value =
                serde_json::from_slice(&packet.payload).expect("the payload is the message");
            assert_eq!(payload["type"], "pause_state");
        }
    }

    /// The interview starts once, when the candidate joined.
    ///
    /// Stamping a second `Instant::now()` after the room and the Gemini session
    /// came up put this process a second or two behind the tab for the rest of
    /// the interview, and the browser announces the round boundary exactly once
    /// off its own clock. The gap has to be absorbed by a tolerance nobody can
    /// justify, or removed here. Read as source because the two stamps are only
    /// distinguishable in a room that really connected.
    #[test]
    fn the_interview_clock_starts_when_the_candidate_joined() {
        let source = include_str!("livekit.rs");
        let opener = source
            .split("async fn open_session")
            .nth(1)
            .expect("open_session is still defined here");
        let bound = opener
            .split_once("let started_at =")
            .expect("open_session still stamps the interview clock")
            .1
            .lines()
            .next()
            .unwrap_or_default();
        assert!(
            bound.contains("setup_began"),
            "the clock must start where the candidate's does, not after setup: {bound}"
        );
    }

    /// The checklist is redrawn for a change the candidate can see, and for
    /// nothing else.
    ///
    /// Evidence for a phase they never reached is recorded but never shown, so
    /// keying the publish on the evidence count sends a message whose phase
    /// list is identical to the one already on screen. The browser unhides the
    /// checklist on every `framework_state` it receives, so the first such
    /// message reveals an empty, wholly unticked list before the candidate has
    /// banked anything.
    #[test]
    fn the_checklist_is_republished_only_when_it_would_look_different() {
        let mut state = RuntimeState::default();
        let skip = serde_json::json!({
            "phase": "optimizations",
            "source": "session_timing",
            "kind": "skipped",
            "confidence": 0,
            "summary": "the session ended before optimizations",
        });

        let shown_before = framework_progress(&state);
        record_framework_evidence(&mut state, &skip).expect("evidence should record");
        assert_eq!(
            state.framework_evidence.len(),
            1,
            "the skip is still recorded for the report"
        );
        assert_eq!(
            framework_progress(&state),
            shown_before,
            "a skip changes nothing on screen, so it must not trigger a redraw"
        );

        // And the rule the publish is keyed on, which the assertion above
        // cannot see: same phases means no redraw, a new phase means one.
        assert!(
            !checklist_changed(&shown_before, &state),
            "a skip leaves the checklist looking exactly as it did"
        );
        record_framework_evidence(
            &mut state,
            &serde_json::json!({
                "phase": "repeat",
                "source": "candidate_speech",
                "kind": "observed",
                "confidence": 80,
                "summary": "restated the inputs and outputs",
            }),
        )
        .expect("evidence should record");
        assert!(
            checklist_changed(&shown_before, &state),
            "a phase the candidate reached is a new tick and has to be sent"
        );
    }

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
            Some(
                r#"{"problemId":"merge-intervals","durationMin":30,"interviewLoop":"coding_only","interviewProfile":{"role":"Platform engineer","seniority":"staff","targetCompany":"Example Co"}}"#,
            ),
        );

        assert_eq!(boot.problem.id, "merge-intervals");
        assert_eq!(boot.duration_min, 30);
        assert_eq!(boot.interview_loop, crate::agent::InterviewLoop::CodingOnly);
        assert_eq!((boot.coding_minutes, boot.behavioral_minutes), (30, 0));
        assert_eq!(boot.profile.role, "Platform engineer");
        assert_eq!(boot.profile.target_company, "Example Co");
        assert!(boot.instructions.contains("candidate selected staff"));
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
        let evidence = execute_tool_call(
            &mut state,
            &GeminiFunctionCall {
                id: "3".to_string(),
                name: TOOL_RECORD_FRAMEWORK_EVIDENCE.to_string(),
                args: serde_json::json!({
                    "phase":"algorithm", "source":"candidate_speech", "kind":"observed",
                    "confidence":90, "summary":"Candidate explained the invariant."
                }),
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
        assert_eq!(evidence["result"]["phase"], "algorithm");
        assert_eq!(state.framework_evidence.len(), 1);
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

        // Derived, not typed out: this test froze the threshold at "16 seconds"
        // and failed the day the threshold moved, which is the one change it
        // has nothing to say about.
        let past_silence = Duration::from_secs_f64(crate::agent::SILENCE_THRESHOLD_S + 1.0);
        activity.last_code_change = now - past_silence;
        activity.last_user_speech = now - past_silence;
        activity.last_agent_speech = now - past_silence;
        activity.last_nudge = now - Duration::from_secs_f64(crate::agent::SILENCE_COOLDOWN_S + 1.0);

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

    /// The ceiling is a boundary, so both sides of it are named. One attempt
    /// short
    /// of the limit still restarts; the limit itself does not, and neither does
    /// the
    /// count past it that a wrong comparison would let through.
    ///
    /// Every case here uses a socket that died young, because a socket that
    /// lived
    /// clears the counter and there would be no ceiling left to test.
    #[test]
    fn the_restart_ceiling_admits_the_last_attempt_and_refuses_the_one_after() {
        let young = Duration::from_secs(1);

        let mut last = GEMINI_RESTART_LIMIT - 1;
        assert!(
            take_restart_attempt(&mut last, young),
            "the attempt below the ceiling is the one a flaky endpoint gets"
        );
        assert_eq!(last, GEMINI_RESTART_LIMIT);

        let mut at_limit = GEMINI_RESTART_LIMIT;
        assert!(!take_restart_attempt(&mut at_limit, young));
        assert_eq!(
            at_limit, GEMINI_RESTART_LIMIT,
            "a refused attempt is not a spent one"
        );
    }

    /// The bug this exists for: Gemini's own connection cap closes a healthy
    /// socket
    /// every ten minutes or so, and counting those against the ceiling turned a
    /// budget for a failing endpoint into a limit on how long an interview
    /// could
    /// run. A socket that carried a working conversation costs nothing.
    #[test]
    fn a_socket_that_lived_clears_what_earlier_failures_spent() {
        let mut restarts = GEMINI_RESTART_LIMIT;

        assert!(take_restart_attempt(&mut restarts, HEALTHY_GEMINI_SOCKET));

        assert_eq!(
            restarts, 1,
            "the healthy socket resets the run, and this attempt is the first of the next one"
        );
    }

    /// The boundary of that reset, from the other side. One second short of
    /// healthy
    /// is still a failing socket, and it must not refill the budget.
    #[test]
    fn a_socket_that_died_just_short_of_healthy_still_spends() {
        let mut restarts = 3;

        assert!(take_restart_attempt(
            &mut restarts,
            HEALTHY_GEMINI_SOCKET - Duration::from_secs(1)
        ));

        assert_eq!(restarts, 4);
    }

    /// Exactly one attempt per restart. Started away from zero on purpose: at
    /// zero
    /// a counter that multiplied instead of adding would look identical to one
    /// that
    /// added.
    #[test]
    fn each_restart_spends_one_attempt() {
        let young = Duration::from_secs(1);
        let mut restarts = 3;

        assert!(take_restart_attempt(&mut restarts, young));
        assert_eq!(restarts, 4);

        assert!(take_restart_attempt(&mut restarts, young));
        assert_eq!(restarts, 5);
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
}
