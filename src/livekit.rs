//! The agent's side of a room: join, stream the candidate's audio and video to
//! Gemini, publish Gemini's audio back, and produce the report when the
//! interview ends.
//!
//! Still the largest module in the tree, so here is its shape.
//!
//! Seven regions are out. `media` holds the buffers, pumps and codecs that
//! carry audio and video in both directions, and `turn` holds the state a turn
//! is made of, which explains what stayed behind with the loop. `rooms` is the
//! only region that talks to LiveKit over HTTP rather than over the room's
//! event stream, `report` is the packet an ending interview produces, and
//! `session` is everything that hangs off one inbound Gemini event or one
//! outbound text: the handlers, the tool dispatch and the turn procedures that
//! end a turn. `packets` is what the browser sends, from the Thinking button's
//! hold to the ending an `end_interview` control writes the report from, and
//! `transcripts` is the writer that sends the transcript to the page.
//!
//! What is left is the room and the loop that drives it:
//!
//! - `run_room` and `join_room`: the lifecycle, and the only entry point.
//! - Session setup and restart (`take_restart_attempt` through `open_session`):
//!   what runs once before the loop, and what the loop calls when the Gemini
//!   socket dies under it.
//! - The loop's three sources, each drained by a handler elsewhere:
//!   `handle_data_packet` in `packets`, `handle_gemini_event` in `session`,
//!   `handle_media_event` in `media`.
//!
//! Session setup is the region that looks separable and is not, which is worth
//! writing down so the next reader does not spend the afternoon finding out.
//! `open_session` and `replace_gemini_session` name `InterviewContext`,
//! `GeminiEventContext`, `OutputAudio`, `CandidateMedia` and `TurnState`, and
//! call `join_room`, `close_turns`, `cut_off_turn`, `set_agent_state`,
//! `publish_interviewer_state` and `candidate_bootstrap`. Moving it out moves
//! the loop's own vocabulary with it, and what is gained is a file boundary
//! rather than a seam.
//!
//! `session` is the one region that came out naming more of this file than the
//! four the others each named, and the only one this file names back: seven
//! each way, in the two `use` blocks below. It earns the boundary a different
//! way. The loop reaches into it at exactly one point, `GeminiEventContext`,
//! which is the borrow handed over for the length of one event and handed
//! straight back; the names on both lists are what that one handover needs,
//! not fourteen separate threads between the two halves. It is a file boundary
//! rather than a layer, and saying so is the point of this paragraph.
//! `packets` crosses at the same handover, one packet at a time, and reaches
//! back through `super` for what the ending needs.

use std::ops::ControlFlow;
use std::sync::Arc;
use std::time::{Duration, Instant};

use ::livekit::DisconnectReason;
use ::livekit::ParticipantKind;
use ::livekit::prelude::{DataPacket, RemoteParticipant, Room, RoomEvent, RoomOptions};

use crate::agent::{
    INTERIM_CONTEXT_NOTES, InterimReviewInput, ModelInputKind, RuntimeState, WATCH_TICK_S,
    apply_data_event_at, code_head, interim_review_prompt, parse_participant_metadata,
    record_interim_notes, transcript_tail, unreviewed_from, with_timer,
};
use crate::config::AgentConfig;
use crate::runtime::TOPIC_CONTROL;
use packets::{apply_data_packet, end_through_control, handle_data_packet};
use room_link::RoomLink;

/// How long past the interview's nominal duration the agent waits before ending
/// it itself. The browser normally ends the interview; this only has to catch
/// the case where it never does, so it is generous rather than tight.
const INTERVIEW_DEADLINE_GRACE: Duration = Duration::from_secs(120);

/// How long after the room opens the process ends the interview itself.
///
/// The planned minutes plus the grace, and a function rather than an expression
/// inside the loop so both terms are reachable from a test: this is what bounds
/// a metered Gemini session when a candidate closes the tab, so an arithmetic
/// slip here is billed for, not noticed.
fn interview_hard_deadline(duration_min: u32) -> Duration {
    Duration::from_secs(u64::from(duration_min) * 60) + INTERVIEW_DEADLINE_GRACE
}

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
    GeminiEvent, GeminiKeys, GeminiLiveSession, generate_interim_review_with_keys,
    generate_phase_judgment_with_keys, live_session_with_keys,
};
use crate::runtime::{AGENT_NAME, RuntimeBootstrap, agent_identity};
use crate::token::{LivekitTokenInput, livekit_token};

mod board;
mod floor;
mod latency;
mod media;
mod packets;
mod report;
mod room_link;
mod room_service;
mod rooms;
mod session;
pub(crate) mod tasks;
mod timing;
mod transcripts;
mod turn;

// The room half reaches back for these on every inbound event, which is what
// makes the split a file boundary rather than a layer: the select loop owns the
// room and hands one borrow of it over for the length of one Gemini event.
pub use session::execute_tool_call;
use session::{
    ContinuityScope, GeminiEventContext, TestRunLine, TurnCause, close_turns, cut_off_turn,
    handle_gemini_event, log_clock, prompt_fields, prompt_line, publish_control,
    send_model_context, send_model_context_scoped, send_model_text, send_model_text_scoped,
    send_wrap_up_and_wait, set_agent_state,
};

use board::{Board, MAX_BOARD_BYTES, handle_board_event};
use report::{freeze_assessment, generate_report_bounded};
use rooms::{evict_duplicate_agent, isolate_local_agent};

// Re-exported rather than merely used: `web::setup` calls this to check the
// credentials a Setup form was given, and it names it through this module.
pub(crate) use rooms::validate_livekit_credentials;

// Room cleanup runs on the web server, not in the agent. The allow goes when
// the peer sweeper calls it.
#[allow(dead_code, unused_imports)]
pub(crate) use room_service::delete_room;

use media::*;
use turn::*;

const GEMINI_OUTPUT_AUDIO_SAMPLE_RATE: u32 = 24_000;
/// How many times in a row the interview may be put back on a fresh Gemini
/// socket after a close that arrived too quickly to be Gemini's ordinary
/// connection cap. In a row, because `take_restart_attempt` clears the count
/// for any socket that lived past `HEALTHY_GEMINI_SOCKET`: this bounds a
/// failing endpoint, not the length of an interview.
pub(crate) const GEMINI_RESTART_LIMIT: usize = 8;

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
/// How long a replacement waits on one reconnect notice before retrying
/// without it. The notice explains the gap; a room that is going down must not
/// widen it.
const RECONNECT_NOTICE_LIMIT: Duration = Duration::from_secs(1);
/// Readiness follows an already paid greeting and is required to start the
/// browser clock. Unlike a best-effort reconnect notice, it can wait out a slow
/// publish. First open (30s), maximum greeting wait (120s), this publish (15s),
/// shutdown (2s) and failure notice (1s) leave headroom in the browser's 180s
/// wait.
const STARTUP_READY_PUBLICATION_LIMIT: Duration = Duration::from_secs(15);
/// Publication must not turn a busy room into another unbounded event buffer.
const STARTUP_READY_EVENT_LIMIT: usize = 256;
/// The reasons a reconnect notice may give. `web/lib.js` words each one, and a
/// browser test reads these lines, so a reason renamed on one side fails there
/// instead of quietly falling back to the generic banner.
const RECONNECT_QUOTA: &str = "quota";
const RECONNECT_RETRYING: &str = "retrying";
const LIVEKIT_AGENT_STATE: &str = "lk.agent.state";
const AGENT_STATE_LISTENING: &str = "listening";
const AGENT_STATE_SPEAKING: &str = "speaking";

/// A reply is owed and has gone `REPLY_WAIT_SHOWN` without starting, or has
/// stopped producing after its audio ran out. The
/// browser already labels this state; nothing published it before issue 95,
/// so a stalled interviewer and one listening on purpose looked the same.
const AGENT_STATE_THINKING: &str = "thinking";
const DUPLICATE_AGENT_ISOLATION_ATTEMPTS: usize = 20;
const WRAP_UP_WAIT: Duration = Duration::from_secs(8);

/// Below this, a queued turn is not a wait anyone experiences, and saying so
/// costs a log line per turn that reads as zero.
const NOTABLE_PLAYOUT_BACKLOG: Duration = Duration::from_millis(500);
/// Covers the normal report attempt, its schema repairs, and bounded transient
/// retries. The candidate is watching a spinner, so this is the point where
/// waiting stops being worth more than an honest incomplete report.
///
/// How many calls that pays for is asserted rather than described, by
/// `report_network_budget_covers_every_repair_and_retry_per_generation`: the
/// sentence that used to give the count here was already naming a budget
/// `src/gemini.rs` no longer had.
pub(super) const REPORT_TIMEOUT: Duration = Duration::from_secs(125);
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
#[derive(Debug, Default, Clone, Copy)]
struct CandidatePresence {
    left_at: Option<Instant>,
}

enum CandidatePresenceChange {
    Left(Instant),
    Returned(Instant),
}

impl CandidatePresence {
    fn left(&mut self, now: Instant) {
        self.left_at.get_or_insert(now);
    }

    fn returned(&mut self) {
        self.left_at = None;
    }

    fn projected(&self, changes: impl Iterator<Item = CandidatePresenceChange>) -> Self {
        let mut presence = *self;
        for change in changes {
            match change {
                CandidatePresenceChange::Left(at) => presence.left(at),
                CandidatePresenceChange::Returned(at) if !presence.gave_up(at) => {
                    presence.returned()
                }
                CandidatePresenceChange::Returned(_) => {}
            }
        }
        presence
    }

    /// Present, or absent for less than the grace, both mean carry on.
    fn gave_up(&self, now: Instant) -> bool {
        self.deadline(CANDIDATE_ABSENCE_LIMIT)
            .is_some_and(|deadline| now >= deadline)
    }

    /// When a grace of `limit` runs out, if the candidate is away. The report
    /// recovery wait holds the same rule with a shorter limit.
    fn deadline(&self, limit: Duration) -> Option<Instant> {
        self.left_at.map(|left| left + limit)
    }
}

/// Receive time survives startup and ready publication; replay is not a new
/// departure or return, and cannot restart the candidate's presence grace.
type TimedRoomEvent = (Instant, RoomEvent);

struct RoomEventReader<Event = RoomEvent> {
    events: tokio::sync::mpsc::UnboundedReceiver<Event>,
    deferred: std::collections::VecDeque<(Instant, Event)>,
}

impl<Event> From<tokio::sync::mpsc::UnboundedReceiver<Event>> for RoomEventReader<Event> {
    fn from(events: tokio::sync::mpsc::UnboundedReceiver<Event>) -> Self {
        Self {
            events,
            deferred: Default::default(),
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

    async fn recv(&mut self) -> Option<Event> {
        self.recv_timed().await.map(|(_, event)| event)
    }

    fn try_recv(&mut self) -> Result<Event, tokio::sync::mpsc::error::TryRecvError> {
        match self.deferred.pop_front() {
            Some((_, event)) => Ok(event),
            None => self.events.try_recv(),
        }
    }
}

async fn next_timed_room_event(
    events: &tokio::sync::Mutex<RoomEventReader>,
) -> Option<TimedRoomEvent> {
    events.lock().await.recv_timed().await
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

/// A checkpoint can be accepted yet restore a conversation that fails again.
/// Once that happens, rebuild locally until output proves recovery worked.
///
/// `resumed` describes the socket now open and only a connect writes it: a
/// resumed socket that answers once and then dies young is still a failed
/// resumption, so completed output lifts the suppression without forgetting
/// where the open socket came from.
///
/// The answer to a recovery briefing proves nothing either way. Every
/// replacement asks for one, so counting it would let a run of sockets that
/// each answer their briefing and die reset the budget forever. It is the
/// first turn to end after the briefing (`recovery_reply_pending`) rather than
/// a reply tagged with the briefing's cause, because a tool call in that answer
/// retags the continuation that completes it.
///
/// Judged by how long the socket stayed up, not by the debt-adjusted age the
/// restart budget uses: a resumed socket that carried minutes of interview and
/// was replaced owing a reply did not fail at its checkpoint.
#[derive(Default)]
struct ResumeRecovery {
    resumed: bool,
    suppressed: bool,
}

impl ResumeRecovery {
    /// Judged on every replacement, whether or not a handle exists to use.
    fn allow_resume(&mut self, age: Duration) -> bool {
        if age >= HEALTHY_GEMINI_SOCKET {
            self.suppressed = false;
        } else if self.resumed {
            self.suppressed = true;
        }
        !self.suppressed
    }

    fn connected(&mut self, resumed: bool) {
        self.resumed = resumed;
    }

    fn note_event(
        &mut self,
        restarts: &mut usize,
        state: &mut RuntimeState,
        activity: &RuntimeActivity,
        event: &GeminiEvent,
    ) {
        // Whichever way the briefing's turn ends, silent or cut off by the
        // candidate, it is over, and the next reply is not its answer.
        if session::ends_turn(event) && std::mem::take(&mut state.recovery_reply_pending) {
            return;
        }
        if completed_live_reply(activity, event) {
            *restarts = 0;
            self.suppressed = false;
        }
    }
}

/// The agent's output has settled: nothing generating, and nothing left in the
/// LiveKit playout queue. The floor alone is not enough, because it is stamped
/// once when a turn completes and the queue drains on its own afterwards.
///
/// Two things wait on this. The goodbye is over when it is true, and a `GoAway`
/// may replace the transport when it is true: replacing Gemini interrupts its
/// output queue, so anything still in flight would be cut.
fn output_settled(floor: Floor, audio_playing: bool) -> bool {
    floor != Floor::Speaking && !audio_playing
}

/// The held half of a `GoAway`: the advisory arrives whenever Gemini decides,
/// and the socket may only be replaced once the output has settled, so the two
/// are separated by however long the current turn has left.
///
/// Kept as a type rather than a bare `bool` because four places in the room
/// loop touch the same advisory, and the one that forgets a rule is how it ends
/// up either cutting a reply or never being spent at all.
#[derive(Default)]
struct DeferredRestart(bool);

impl DeferredRestart {
    /// An advisory is held and not yet spent.
    fn is_armed(&self) -> bool {
        self.0
    }

    fn can_spend(&self, activity: &RuntimeActivity, now: Instant) -> bool {
        self.is_armed() && !activity.incoming_speech_active(now)
    }

    /// The candidate is mid-utterance with fresh evidence, which is the case a
    /// speech handoff exists for. Kept apart from [`Self::can_spend`], whose
    /// speech guard holds an ordinary restart back for exactly this case.
    fn handoff_due(&self, activity: &RuntimeActivity, now: Instant) -> bool {
        self.is_armed()
            && activity.candidate_voice_active
            && !activity.candidate_transcription_pending(now)
    }

    /// After speech ends the spend waits out the transcription tail, and no
    /// provider event need follow it, so the watch keeps retrying until then.
    fn watch_due(&self, activity: &RuntimeActivity, stalled: bool, now: Instant) -> bool {
        self.handoff_due(activity, now)
            || (self.can_spend(activity, now)
                && (stalled || activity.candidate_voice_active || activity.candidate_voice_ended()))
    }

    /// Some other path replaced the socket. Leaving the advisory armed would
    /// make the first completed turn on the new socket pay for a second
    /// replacement.
    fn cancel(&mut self) {
        self.0 = false;
    }

    /// A `GoAway` arrived. `true` is the caller's cue to replace the socket
    /// now; otherwise the advisory is held for `take_if_due`.
    ///
    /// Armed first and then spent through `take_if_due`, so the settled rule
    /// lives in one place and this method holds only what it adds to it.
    ///
    /// `reply_pending` is that addition, and the one signal `output_settled`
    /// cannot carry. `Floor::Speaking` is stamped on the first audio chunk, so
    /// a turn that has issued a tool call and produced no audio is still
    /// generating under a `Listening` floor. Without it a `GoAway` landing
    /// there throws the generation away after its tool response has already
    /// gone out on the old socket.
    fn request(
        &mut self,
        floor: Floor,
        audio_playing: bool,
        reply_pending: bool,
        tool_owed: bool,
    ) -> bool {
        self.0 = true;
        !reply_pending && self.take_if_due(floor, audio_playing, tool_owed)
    }

    /// Spends a held advisory at the first settled moment after it.
    ///
    /// Weighs `tool_response_outstanding` and deliberately not the pending
    /// reply that `request` also reads. The two are not the same debt. A
    /// candidate talking over the queued reply ends the turn this was deferred
    /// behind without Gemini ever calling it an interruption, and that same
    /// event stamps a pending reply; holding on it would carry the advisory
    /// across another whole turn until `timeLeft` was spent and the socket
    /// dropped mid-generation, which is the outcome deferring exists to avoid.
    /// A tool response is the opposite case: it is generation already paid for
    /// on this socket, it leaves the floor `Listening` with nothing playing, so
    /// every term above reads settled while the answer is still owed.
    fn take_if_due(&mut self, floor: Floor, audio_playing: bool, tool_owed: bool) -> bool {
        if !self.0 || tool_owed || !output_settled(floor, audio_playing) {
            return false;
        }
        self.0 = false;
        true
    }

    /// `take_if_due` read off the room's own state, which is how every caller
    /// asks it, so the settled rule is read the same way at each.
    fn take_if_settled(
        &mut self,
        activity: &RuntimeActivity,
        audio_playing: bool,
        now: Instant,
    ) -> bool {
        if activity.candidate_transcription_pending(now) {
            return false;
        }
        self.take_if_due(
            activity.floor,
            audio_playing,
            activity.socket.tool_response_outstanding,
        )
    }
}

/// The age a replaced socket is judged by for the restart budget. One that
/// left a reply unanswered is not healthy just because it stayed connected:
/// the watchdog found it silent, or it still owed a reply or was partway
/// through one when a `GoAway` or the server closed it. A provider that closes
/// silent sockets a minute in would otherwise reset the run on every one and
/// never let the budget end the interview.
fn replaced_socket_age(age: Duration, reply_timeout: bool, activity: &RuntimeActivity) -> Duration {
    if reply_timeout || activity.reply_unfinished() {
        Duration::ZERO
    } else {
        age
    }
}

/// The room loop's activity, with the limits the operator configured fixed for
/// the whole interview.
fn runtime_activity(config: &AgentConfig, started_at: Instant, session_id: u64) -> RuntimeActivity {
    RuntimeActivity::for_interview(started_at, config.max_interim_reviews, session_id)
        .with_reply_timeout(Duration::from_secs(u64::from(
            config.gemini_reply_timeout_s,
        )))
        .with_first_output_deadline(Duration::from_secs(u64::from(config.gemini_first_output_s)))
}

// Latency includes the opening wait. The probe has already observed output,
// even if publishing ready delays its delivery to the room loop: the watchdog
// must judge generation progress rather than ask for the first output again.
fn ready_runtime_activity(
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
            .saturating_sub(u64::try_from(started_at.elapsed().as_millis()).unwrap_or(u64::MAX)),
    );
    activity
}

/// Opens a session that remembers nothing, retrying while the budget allows.
///
/// `Err` means credentials are unavailable or retries stopped, ending the
/// interview. Split from `replace_gemini_session` because a loop with its own
/// exit inside an arm of a match inside an arm of a match put the give-up path
/// four levels deep in a function that is otherwise a sequence of steps.
async fn open_cold_session(
    interview: InterviewContext<'_>,
    restarts: &mut usize,
    room: Option<&RoomLink>,
) -> Result<GeminiLiveSession, Box<dyn std::error::Error + Send + Sync>> {
    let deadline = room.is_none().then(|| {
        tokio::time::Instant::from_std(interview.started_at) + crate::gemini::FIRST_OPEN_LIMIT
    });
    retry_cold_open(
        interview.keys,
        restarts,
        deadline,
        || live_session_with_keys(interview.keys, interview.boot, None),
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
    )
    .await
}

/// The loop of `open_cold_session`, with the opener passed in so tests can
/// point it at a local socket.
async fn retry_cold_open<F, Fut, N, NFut>(
    keys: &GeminiKeys,
    restarts: &mut usize,
    deadline: Option<tokio::time::Instant>,
    mut open: F,
    mut notify: N,
) -> Result<GeminiLiveSession, Box<dyn std::error::Error + Send + Sync>>
where
    N: FnMut(serde_json::Value) -> NFut,
    NFut: Future<Output = ()>,
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<GeminiLiveSession, Box<dyn std::error::Error + Send + Sync>>>,
{
    // Announced alongside the attempt it describes rather than ahead of it, so
    // a slow room does not delay the retry; still finished before the loop
    // returns, so it cannot land after the notice that clears the banner.
    let mut attempt_notice = None;
    loop {
        let announce = attempt_notice.take().map(&mut notify);
        let (opened, ()) = tokio::join!(open(), async {
            if let Some(announce) = announce {
                announce.await;
            }
        });
        let (reason, until) = match opened {
            Ok(session) => return Ok(session),

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
        tokio::time::sleep_until(until).await;

        // The estimate has run out, but the attempt it led to can take a
        // connect and a setup timeout, so the page drops the number rather than
        // show it for half a minute.
        attempt_notice = Some(reconnect_wait(reason, None));
    }
}

/// The bare notice; `reconnect_wait` adds why a replacement is waiting.
fn interviewer_state(reconnecting: bool) -> serde_json::Value {
    serde_json::json!({ "type": "interviewer_state", "reconnecting": reconnecting })
}

fn startup_state(error: Option<&(dyn std::error::Error + 'static)>) -> serde_json::Value {
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

fn startup_ready_notice(started_at: Instant, wall_clock: Option<u64>) -> serde_json::Value {
    let mut notice = startup_state(None);
    notice["startedAt"] = wall_clock.into();

    // Only elapsed time crosses clock domains. The wall-clock stamp identifies
    // the original opening but cannot be subtracted from a browser's clock.
    notice["elapsedMs"] = u64::try_from(started_at.elapsed().as_millis())
        .unwrap_or(u64::MAX)
        .into();
    notice
}

/// The normal opening generation is the probe, not a second paid turn. Setup
/// alone proves nothing, and no watchdog or replacement runs before it answers.
async fn probe_live_output(
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
                        if let Some(received_at) = gemini.buffered_startup_output(deadline.into_std())? {
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

struct StartupContext<'a> {
    media: &'a mut CandidateMedia,
    candidate_identity: &'a str,
    candidate_video: bool,
    deferred_events: &'a mut Vec<TimedRoomEvent>,
    room_closed: &'a mut bool,
}

enum StartupInput {
    Room(Option<RoomEvent>),
    ParticipantLeft(RoomEvent),
    Audio(Option<::livekit::webrtc::audio_frame::AudioFrame<'static>>),
    Video(Option<::livekit::webrtc::video_frame::BoxVideoFrame>),
}

trait StartupInputs: Send {
    fn next(&mut self, media: &mut CandidateMedia) -> impl Future<Output = StartupInput> + Send;
}

impl StartupInputs for &tokio::sync::Mutex<RoomEventReader> {
    async fn next(&mut self, media: &mut CandidateMedia) -> StartupInput {
        next_startup_input(media, self).await
    }
}

async fn next_startup_input(
    media: &mut CandidateMedia,
    events: &tokio::sync::Mutex<RoomEventReader>,
) -> StartupInput {
    tokio::select! {
        event = async { events.lock().await.recv().await } => match event {
            Some(event @ RoomEvent::ParticipantDisconnected(_)) => StartupInput::ParticipantLeft(event),
            event => StartupInput::Room(event),
        },
        frame = next_audio_frame(&mut media.audio), if media.audio.is_some() => StartupInput::Audio(frame),
        frame = next_video_frame(&mut media.video), if media.video.is_some() => StartupInput::Video(frame),
    }
}

fn startup_output_limit(config: &AgentConfig) -> Duration {
    Duration::from_secs(u64::from(
        config
            .gemini_first_output_s
            .max(crate::config::DEFAULT_GEMINI_FIRST_OUTPUT_S)
            .min(config.gemini_reply_timeout_s),
    ))
}

/// The notice a replacement shows while it waits on `reason`. `None` is the
/// attempt itself, whose length no one knows.
fn reconnect_wait(reason: &str, wait: Option<Duration>) -> serde_json::Value {
    let mut notice = interviewer_state(true);
    notice["reason"] = reason.into();
    if let Some(wait) = wait {
        notice["waitSeconds"] = report::whole_seconds(wait).into();
    }
    notice
}

/// How an interview's Live session ended, as its summary line spells it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LiveOutcome {
    Ok,
    Error,
    GeminiUnreachable,
    Billing,
}

impl LiveOutcome {
    /// A project out of credit is named wherever the failure surfaced, since
    /// it is the one an operator fixes by paying rather than by waiting, and
    /// the one that looks like any other silence from the candidate's side.
    fn from_error(error: &(dyn std::error::Error + 'static), otherwise: Self) -> Self {
        if crate::gemini::is_billing_failure(error) {
            Self::Billing
        } else {
            otherwise
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Error => "error",
            Self::GeminiUnreachable => "gemini_unreachable",
            Self::Billing => "billing",
        }
    }
}

/// Puts the interview back on a new Gemini socket, or reports that it cannot
/// be.
///
/// `Break` means the interview is over. Nothing restarts it: the dispatcher
/// spawns one task per room and drops the slot when it returns. It ends through
/// `end_through_control` all the same, so the report is written from the
/// session this process holds. A budget spent on replacements that never
/// answered is a provider outage, and the candidate who sat through it still
/// did the work; leaving with no report handed them a live room and nothing.
///
/// That is why a missing or rejected resumption handle is not the end. Resuming
/// keeps what was said; a cold session keeps only what the system instruction
/// carries, which is the problem, the rubric and the round, plus the editor
/// snapshot handed over below. Losing the conversation is a worse interview.
/// Losing the interviewer is no interview.
///
/// The reason the socket closed came over it and is logged by the reader task.
/// The lines below tie that to the room.
async fn replace_gemini_session(
    room: &RoomLink,
    context: &mut GeminiEventContext<'_>,
    interview: InterviewContext<'_>,
    loops: &mut RoomLoop,
    trigger: Trigger,
    speech_handoff: Option<session::SpeechHandoff>,
) -> Result<ControlFlow<()>, Box<dyn std::error::Error + Send + Sync>> {
    // The gap the candidate sat through starts when the close is handled, not
    // when the new socket is asked for: the budget checks and the resume
    // attempt that fails before a cold open are part of it.
    let rotation_started = Instant::now();

    // Neither a resume nor a cold open on the same key can get past a project
    // that cannot pay, so the budget is not spent learning that twice more.
    session::finish_turn_timing(
        room.name(),
        context.activity,
        &log_clock(context.state),
        timing::TimingOutcome::Replaced,
        &mut std::io::stderr().lock(),
    );
    if context.gemini.closed_for_billing(interview.keys) {
        context.activity.live_exit = LiveOutcome::Billing;
        eprintln!(
            "Gemini closed the session because the project cannot pay; ending interview room={}",
            interview.boot.room_name
        );
        leave_room(room).await;
        return Ok(ControlFlow::Break(()));
    }

    // Reading the handle also records closing-key failures for cold opens and
    // reporting, even when the handle or the restart budget cannot be used.
    let handle = context.gemini.recovery_handle(interview.keys);
    let age = replaced_socket_age(
        context.gemini.age(),
        trigger == Trigger::Watchdog,
        context.activity,
    );
    if !take_restart_attempt(&mut loops.restarts, age) {
        context.activity.live_exit = LiveOutcome::GeminiUnreachable;
        eprintln!(
            "Gemini closed {} sockets in a row without one of them lasting; ending interview with a report room={}",
            loops.restarts, interview.boot.room_name
        );
        return end_without_interviewer(room, context, interview, loops).await;
    }

    // Judged before the handle is looked at: `Option::filter` skips its closure
    // on `None`, which would leave suppression stale across a replacement that
    // had no handle to offer.
    let allow_resume = loops.resume_recovery.allow_resume(context.gemini.age());
    let had_handle = handle.is_some();
    let handle = handle.filter(|_| {
        allow_resume
            && speech_handoff.as_ref().is_none_or(|handoff| {
                context
                    .gemini
                    .checkpoint_age()
                    .is_some_and(|age| age <= handoff.checkpoint_after.elapsed())
            })
    });
    let handle_offered = handle.is_some();

    // The cold-rebuild line tells the two causes of a 1011 run apart: a resumed
    // conversation that keeps failing stops failing once it is rebuilt, while
    // input that fails on any socket fails the rebuild too.
    if loops.resume_recovery.suppressed {
        eprintln!(
            "{}; rebuilding from local state room={}",
            if loops.resume_recovery.resumed {
                "Gemini resumed socket was replaced young, so resumption is suppressed"
            } else {
                "Gemini cold socket was replaced young"
            },
            interview.boot.room_name
        );
    }

    // Said before the attempt, not after it: the whole point is to cover the
    // gap, and the gap starts here. Connect and setup are bounded at fifteen
    // seconds each, and for that long the candidate is talking to a socket that
    // is gone.
    publish_control(room, &interviewer_state(true), "the reconnect notice").await;

    // Closes the writer half and the reader task. A socket that is already gone
    // errors on the close, and that error says nothing the caller can act on;
    // one being replaced ahead of its `GoAway` is still live and this is the
    // orderly hang-up. Ignored either way for that reason. A socket that
    // resumed and was offered nothing new still holds the checkpoint it
    // inherited, whose age this process does not know. A checkpoint that is not
    // offered is not the one the replacement starts from, so it is not named.
    let checkpoint_age = match (&handle, context.gemini.checkpoint_age()) {
        (None, _) => "none".to_string(),
        (Some(_), Some(age)) => format!("{}s", age.as_secs()),
        (Some(_), None) => "inherited".to_string(),
    };
    let _ = context.gemini.shutdown().await;
    session::drain_live_usage(room, context);

    // A handle is worth one try and no more: one the server refuses fails
    // identically every time, so retrying it under the budget would spend the
    // whole budget learning the same thing. Both ways of not having a resumable
    // session -- no handle, and a handle that was refused -- leave a `None` for
    // the cold start below to answer, which is why neither is a branch of its
    // own here.
    let resumed_session = match handle {
        Some((key, handle)) => {
            live_session_with_keys(interview.keys, interview.boot, Some((&key, &handle)))
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
    let fallback = fallback_reason(
        resumed,
        speech_handoff.is_some(),
        allow_resume,
        had_handle,
        handle_offered,
    );
    let handover = handover_kind(fallback);
    let session = match resumed_session {
        Some(session) => Ok(session),
        None => open_cold_session(interview, &mut loops.restarts, Some(room)).await,
    };
    let session = match session {
        Ok(session) => session,
        Err(error) => {
            let outcome = LiveOutcome::from_error(error.as_ref(), LiveOutcome::GeminiUnreachable);
            context.activity.live_exit = outcome;

            // A project that cannot pay would be billed for the report too, so
            // it leaves the way the billing check above does.
            if outcome == LiveOutcome::Billing {
                eprintln!(
                    "Gemini could not be reached: the project cannot pay; ending interview room={}",
                    interview.boot.room_name
                );
                leave_room(room).await;
                return Ok(ControlFlow::Break(()));
            }
            eprintln!(
                "Gemini could not be reached; ending interview with a report room={}",
                interview.boot.room_name
            );
            return end_without_interviewer(room, context, interview, loops).await;
        }
    };
    context
        .state
        .evidence_ledger
        .record_model_input(ModelInputKind::LiveSetup, &interview.boot.instructions);

    *context.gemini = session;
    context.media.presence_frame_sent = false;
    loops.resume_recovery.connected(resumed);
    context.activity.live_socket += 1;
    context.activity.reset_context_observations(resumed);
    context.activity.sync_cut_plausible(context.state);

    // `rotation_gap_ms` ends at setup, before the briefing goes out. The
    // first-audio line follows only when the briefing asked for a reply, and
    // counts the briefing and any board resend too, so the difference between
    // them is what those and the model's reply cost. Named apart from the
    // `gap_ms` the floor lines carry, which measures something else.
    let connected = Instant::now();
    let gap = connected.saturating_duration_since(rotation_started);
    let (owed, debt, owed_prompt) =
        hand_over(context.state, context.activity, context.output_audio);

    // The one line that ties a replacement to what the new socket is missing:
    // how old the checkpoint it resumed from is, what was owed, and what the
    // local record holds that the checkpoint may predate.
    eprintln!(
        "replacement: at={} resumed={resumed} {} rotation_gap_ms={} checkpoint_age={checkpoint_age} owed={owed} ({debt}) owed_chars={} editor_chars={} {} room={}",
        log_clock(context.state),
        replacement_fields(trigger, handover, fallback),
        crate::agent::saturating_ms(gap),
        owed_prompt
            .as_deref()
            .map_or(0, |prompt| prompt.chars().count()),
        context.state.code.chars().count(),
        prompt_fields(context.state, context.activity),
        interview.boot.room_name
    );
    close_turns(room, context);
    set_agent_state(room, context.agent_state, AGENT_STATE_LISTENING);
    publish_control(room, &interviewer_state(false), "the reconnected notice").await;
    eprintln!("{}", replacement_notice_line(handover, room.name()));
    let spoke = if let Some(handoff) = speech_handoff {
        let listening = match session::send_listening_handoff(
            context.gemini,
            context.state,
            &handoff,
            resumed,
            owed_prompt.as_deref(),
        )
        .await
        {
            Ok(()) => {
                context.activity.commit_silent_editor(context.state);
                true
            }
            Err(error) => {
                context.state.needs_cold_brief = true;
                eprintln!(
                    "speech-handoff briefing failed ({error}); waiting for the close to be reported"
                );
                false
            }
        };

        // Still owed after the handoff: the next output clears it, and a socket
        // that drops first names it in its own briefing. A reply owed to
        // candidate speech has no prompt text and is still owed.
        if owed || owed_prompt.is_some() {
            context
                .activity
                .owe_prompt(Instant::now(), owed_prompt.clone());

            // Only a socket that was told to keep listening, and owes a reply
            // to what it was listening to: one that never got the note has no
            // prefix to answer, and one owing nothing has no reply to ask for.
            if listening {
                context.activity.arm_handoff_reply(Instant::now());
            }
        }
        false
    } else {
        brief_replacement(
            context.gemini,
            context.state,
            context.activity,
            if resumed {
                Replacement::Resumed { owed }
            } else {
                Replacement::Cold { owed }
            },
            owed_prompt.as_deref(),
        )
        .await
    };

    // A cold session remembers none of the drawing, and the briefing's text
    // cannot carry it, so the board follows the briefing and the session that
    // speaks next has seen it.
    if !resumed
        && context.state.interview_mode.is_whiteboard()
        && let Err(error) = board::resend(context.board, context.gemini).await
    {
        eprintln!("cold-restart board failed ({error}); waiting for the close to be reported");
    }
    context.activity.begin_rotation_watch(
        spoke,
        turn::RotationWatch {
            started: rotation_started,
            connected,
            resumed,
            candidate_spoke_at: None,
        },
    );
    if spoke {
        eprintln!(
            "{}",
            prompt_line(
                context.state,
                context.activity,
                "kind=briefing",
                interview.boot.room_name,
            )
        );
    }
    Ok(ControlFlow::Continue(()))
}

/// The Live socket cannot be kept, so the interview ends the way the deadline
/// ends it, with no goodbye to ask for. Already-ended sessions are not ended
/// twice; the room is left all the same.
async fn end_without_interviewer(
    room: &RoomLink,
    context: &mut GeminiEventContext<'_>,
    interview: InterviewContext<'_>,
    loops: &mut RoomLoop,
) -> Result<ControlFlow<()>, Box<dyn std::error::Error + Send + Sync>> {
    // A replacement that gave up after announcing itself would leave the
    // reconnecting notice up over the report. Not `?`: the report matters more
    // than the notice.
    publish_control(
        room,
        &interviewer_state(false),
        "the reconnecting notice's end",
    )
    .await;
    if context.state.ended {
        leave_room(room).await;
    } else {
        end_through_control(
            room,
            context,
            interview,
            INTERVIEWER_UNAVAILABLE,
            &mut loops.interim_review,
        )
        .await?;
    }
    Ok(ControlFlow::Break(()))
}

/// Ends what the dead socket left in flight, and reports whether the new one
/// owes a reply and why, for the replacement log line: the candidate's turn,
/// the number of a prompt still unanswered or `none`, and a tool continuation.
/// The prompt's own text comes back too, so the briefing can name what to
/// answer rather than leave the new socket to guess.
/// Everything a replacement does to the room's own state and nothing it does
/// to the room, so a test drives the same steps production takes.
///
/// Read before `cut_off_turn` clears the pending work: a reply the old socket
/// owed is still owed, and the replacement is the only one left to give it.
/// Whatever was mid-flight died with the socket. The turn ids have to close
/// for the same reason an interruption closes them: left open, the next thing
/// either party says appends to an utterance that was cut off, and the panel
/// and the report both read the two as one. Neither kind of replacement holds
/// the evidence lines the next watch prompt would otherwise skip: a cold one
/// has seen none, and a resumed one restarts from a checkpoint that may
/// predate the latest of them.
fn hand_over(
    state: &mut RuntimeState,
    activity: &mut RuntimeActivity,
    output_audio: &mut OutputAudio,
) -> (bool, String, Option<String>) {
    let owes_prompt = activity.owes_prompt();
    let owed_prompt = activity.prompt_text.take().filter(|_| owes_prompt);
    let prompt = if owes_prompt {
        activity.prompt_sequence.to_string()
    } else {
        "none".to_string()
    };
    let debt = format!(
        "candidate={} prompt={prompt} tool={} generating={}",
        activity.reply_in_flight(),
        activity.socket.tool_response_outstanding,
        activity.generating,
    );
    let owed = activity.reply_unfinished();
    cut_off_turn(state, activity, output_audio);
    // Late fragments belong to the retired socket, never its replacement.
    state.interviewer_delivery.replace();
    clear_abandoned_socket_work(state, activity);
    activity.evidence_shown = None;
    activity.silent_editor_delivery = None;
    (owed, debt, owed_prompt)
}

/// How a socket was replaced, as far as the briefing sent to it cares.
#[derive(Clone, Copy)]
enum Replacement {
    /// A session that remembers nothing, which always has to be briefed.
    /// `owed` as below: the briefing alone grounds the new socket, and without
    /// the owed-reply request it resumes the round rather than answering the
    /// candidate who is still waiting.
    Cold { owed: bool },
    /// Continued from a resumption handle. `owed` when the old socket owed a
    /// reply it never produced.
    Resumed { owed: bool },
}

impl Replacement {
    fn owed(self) -> bool {
        match self {
            Self::Cold { owed } | Self::Resumed { owed } => owed,
        }
    }
}

/// What made the socket be replaced.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Trigger {
    /// The reply watchdog gave up on a socket that was not answering.
    Watchdog,
    /// The server hung up before an advisory was acted on.
    Close,
    /// A `GoAway` advisory was acted on, held or not.
    GoAway,
}

impl Trigger {
    fn as_str(self) -> &'static str {
        match self {
            Self::Watchdog => "watchdog",
            Self::Close => "close",
            Self::GoAway => "goaway",
        }
    }
}

/// Why a replacement did not resume, or `None` when it did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Fallback {
    None,
    /// The replaced socket had offered no handle.
    NoHandle,
    /// A handle existed, but the replaced socket had been resumed and was
    /// itself replaced young, so resumption is withheld.
    ResumeSuppressed,
    /// The candidate was speaking and no checkpoint post-dated the
    /// cancellation, so the handle was not offered on purpose.
    CheckpointBeforeHandoff,
    /// The handle was offered and the resume did not take.
    ResumeRefused,
}

impl Fallback {
    fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::NoHandle => "no_handle",
            Self::ResumeSuppressed => "resume_suppressed",
            Self::CheckpointBeforeHandoff => "checkpoint_before_handoff",
            Self::ResumeRefused => "resume_refused",
        }
    }
}

/// How a replacement socket was brought up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Handover {
    /// The conversation continued from a resumption handle.
    Resumed,
    /// Rebuilt from local state because the candidate was speaking and no
    /// checkpoint post-dated the cancellation. Resuming from an older one can
    /// replay the reply that was cancelled, so this is the designed fallback.
    ColdHandoff,
    /// Rebuilt from local state without that reason: no handle, a refused
    /// handle, or resumption suppressed after a young resumed socket.
    Degraded,
}

impl Handover {
    fn as_str(self) -> &'static str {
        match self {
            Self::Resumed => "resumed",
            Self::ColdHandoff => "cold_handoff",
            Self::Degraded => "degraded",
        }
    }
}

/// Why a replacement did not resume. The checkpoint rule is the cause only when
/// a handle existed, resumption was allowed, there was a speech handoff, and
/// that rule alone kept the handle from being offered; a handle that was
/// offered and then refused, or never existed, is a failure to resume.
fn fallback_reason(
    resumed: bool,
    speech_handoff: bool,
    allow_resume: bool,
    had_handle: bool,
    handle_offered: bool,
) -> Fallback {
    if resumed {
        Fallback::None
    } else if !had_handle {
        Fallback::NoHandle
    } else if !allow_resume {
        Fallback::ResumeSuppressed
    } else if speech_handoff && !handle_offered {
        Fallback::CheckpointBeforeHandoff
    } else {
        Fallback::ResumeRefused
    }
}

fn handover_kind(fallback: Fallback) -> Handover {
    match fallback {
        Fallback::None => Handover::Resumed,
        Fallback::CheckpointBeforeHandoff => Handover::ColdHandoff,
        _ => Handover::Degraded,
    }
}

/// The fields of the `replacement:` line the soak reads to judge a handover, in
/// the spelling it parses. Kept as `key=value` tokens so a new field never
/// moves the match.
fn replacement_fields(trigger: Trigger, handover: Handover, fallback: Fallback) -> String {
    format!(
        "trigger={} handover={} fallback={}",
        trigger.as_str(),
        handover.as_str(),
        fallback.as_str()
    )
}

/// The line that says how a replacement socket was brought up, printed once per
/// replacement with the room appended.
///
/// For the operator. The soak judges a handover from the structured fields of
/// the `replacement:` line, see [`replacement_fields`], not from this text.
fn replacement_notice(handover: Handover) -> &'static str {
    match handover {
        Handover::Resumed => "Gemini session resumed; the interview continues where it left off",
        Handover::ColdHandoff => {
            "Gemini session restart rebuilt locally; the candidate was speaking and no checkpoint post-dated it, so the interviewer is rebuilding from local transcript, editor and interview state"
        }
        Handover::Degraded => {
            "Gemini session restart degraded; resumption was not used and the interviewer is rebuilding from local transcript, editor and interview state"
        }
    }
}

/// [`replacement_notice`] with the room appended after the matched text, so the
/// soak's literal match still finds it and two rooms rotating together stay
/// apart in the log.
fn replacement_notice_line(handover: Handover, room: &str) -> String {
    format!("{} room={room}", replacement_notice(handover))
}

/// Tells a replacement socket what it cannot know on its own, and reports
/// whether the briefing asked for a reply.
///
/// A resumed checkpoint is the last turn boundary the server marked resumable,
/// so it may predate the latest test run or spoken answer; a cold one knows
/// nothing. Both get the local record. Kept apart from `replace_gemini_session`
/// so a test drives the same branch production does.
async fn brief_replacement(
    gemini: &mut GeminiLiveSession,
    state: &mut RuntimeState,
    activity: &mut RuntimeActivity,
    replacement: Replacement,
    owed_prompt: Option<&str>,
) -> bool {
    // Not `?`, for the reason the watch loop already gives about writes to
    // Gemini: a socket that came up and died again reports the close through
    // `next_event`, where this arm is waiting to restart it. Propagating here
    // would end the interview on the one write the restart exists to make, and
    // the write most likely to meet a socket that is already gone.
    match send_recovery_brief(gemini, state, replacement, owed_prompt).await {
        Ok(true) => {
            activity.commit_silent_editor(state);

            // The briefing is scaffolding, not the event: a socket that drops
            // before answering it still owes what the briefing asked about, and
            // naming the briefing there would nest it, transcript and all,
            // inside the next one.
            activity.mark_recovery_prompted(Instant::now(), owed_prompt);
            state.recovery_reply_pending = true;
            true
        }
        Ok(false) => {
            if !state.needs_cold_brief {
                activity.commit_silent_editor(state);
            }
            false
        }
        Err(error) => {
            eprintln!(
                "connection-recovery briefing failed ({error}); waiting for the close to be reported"
            );
            keep_recovery_debt(state, activity, replacement, owed_prompt);
            false
        }
    }
}

/// A briefing that never reached the socket leaves its debt with whichever
/// socket replaces this one. Teardown already cleared the old socket's pending
/// work, so without this the next replacement is told nothing was owed: a cold
/// briefing is retried as one, and an owed reply is owed again.
fn keep_recovery_debt(
    state: &mut RuntimeState,
    activity: &mut RuntimeActivity,
    replacement: Replacement,
    owed_prompt: Option<&str>,
) {
    if matches!(replacement, Replacement::Cold { .. }) {
        state.needs_cold_brief = true;
    }
    if replacement.owed() {
        activity.owe_prompt(Instant::now(), owed_prompt.map(str::to_string));
    }
}

/// Sends the briefing, and reports whether it asked for a reply.
///
/// A resumed socket gets its update as context. It speaks only when the old
/// socket owed a reply, since otherwise the candidate has the floor. A cold
/// socket always speaks, except during a thinking hold, where its reply would
/// be discarded: it is briefed as context that asks for no answer instead. A
/// resumed socket whose cold predecessor still owed its briefing gets that
/// briefing, since its memory starts from the cold session.
async fn send_recovery_brief(
    gemini: &mut GeminiLiveSession,
    state: &mut RuntimeState,
    replacement: Replacement,
    owed_prompt: Option<&str>,
) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
    // A reply owed during a hold cannot be asked for yet, since it would be
    // dropped; ending the hold asks for it instead, on either kind of socket.
    let owed = replacement.owed();
    let held = state.interviewer_silenced();
    if owed && held {
        hold_owed_reply(state, owed_prompt);
    }
    if matches!(replacement, Replacement::Resumed { .. }) && !state.needs_cold_brief {
        let reply = owed && !held;
        let mut context = crate::agent::resumed_context(state, reply, owed_prompt);

        // A resumed session keeps what a hold dropped in its history. The reply
        // asked for here pays whatever the hold left owed, so it carries it,
        // built by the function whose debt `clear_thinking_debt` clears.
        if reply {
            context = crate::agent::with_thinking_debt(state, &context);
        }
        let context = crate::agent::with_timer(state, context);
        // The briefing carries the whole editor, so the model has now seen it.
        state.code_shown = state.code.clone();
        send_model_context_scoped(
            gemini,
            state,
            ModelInputKind::Turn,
            &context,
            reply.then_some(TurnCause::Recovery),
            ContinuityScope::Resumed,
        )
        .await?;
        if reply {
            state.clear_thinking_debt();
        }
        return Ok(reply);
    }
    if held {
        // The reply the old socket owed was held above, since `hand_over` has
        // already taken the prompt off the activity; it is asked for when the
        // hold ends.
        //
        // The candidate's audio still reaches the new socket during a hold, and
        // it could answer them knowing nothing of the interview. Briefed now,
        // as context that asks for no answer.
        let briefing = crate::agent::with_timer(state, crate::agent::cold_restart(state));
        state.code_shown = state.code.clone();
        send_model_context_scoped(
            gemini,
            state,
            ModelInputKind::Turn,
            &briefing,
            None,
            ContinuityScope::ColdOrCheckpoint,
        )
        .await?;
        // Delivered, so releasing the hold must not brief this socket again.
        state.needs_cold_brief = false;
        return Ok(false);
    }
    let briefing = crate::agent::with_owed_reply(
        crate::agent::cold_restart(state),
        owed.then_some(owed_prompt),
    );
    let briefing = crate::agent::with_timer(state, briefing);
    state.code_shown = state.code.clone();
    send_model_text_scoped(
        gemini,
        state,
        ModelInputKind::Turn,
        TurnCause::Recovery,
        &briefing,
        ContinuityScope::ColdOrCheckpoint,
    )
    .await?;
    state.clear_thinking_debt();
    Ok(true)
}

/// Keeps a reply a replaced socket owed during a hold for the hold's release to
/// ask for. A known event is never replaced by an unknown one: a candidate
/// transcript landing in the hold leaves `hand_over` no prompt text, but the
/// event the hold began on is still the one the reply belongs to.
fn hold_owed_reply(state: &mut RuntimeState, owed_prompt: Option<&str>) {
    if owed_prompt.is_some() || state.owed_reply_on_resume.is_none() {
        state.owed_reply_on_resume = Some(owed_prompt.map(str::to_string));
    }
}

/// Drops work that could only have been completed by the replaced socket.
///
/// A close request follows its tool acknowledgement: without that generation,
/// the loop must not turn a request from the old socket into a closing from the
/// new one. The other two flags are the same kind of debt. A closed socket
/// cannot send the event that clears them, and carrying either into its
/// replacement makes the new interviewer discard output or wait on work it did
/// not create.
fn clear_abandoned_socket_work(state: &mut RuntimeState, activity: &mut RuntimeActivity) {
    activity.turn_timing.reset();
    activity.buffered_startup_output_at = None;
    activity.socket = turn::SocketWork::default();
    state.recovery_reply_pending = false;

    // A provisional request waits on its utterance's end, which the closed
    // socket will not send. A declared hold is the candidate's and survives.
    state.withdraw_thinking_request();
    state.end_requested = false;
}

/// Reads the stretch of interview nobody has assessed yet, off to one side.
///
/// The room loop never waits on this. That is the whole point: the evaluation
/// that used to happen after the candidate stopped talking happens here
/// instead, in a pause, while the loop carries on handling their next word. It
/// is collected on a later watch tick, once the handle reports itself finished,
/// and nothing fails if it never does.
fn spawn_interim_review(
    state: &mut RuntimeState,
    interview: InterviewContext<'_>,
) -> tokio::task::JoinHandle<String> {
    let prompt = take_interim_review_window(state, interview.boot);
    spawn_side_call(SideCallKind::SideCall, prompt, interview)
}

/// Which side call `spawn_side_call` makes. Its label opens the log line a
/// failed call leaves, which `scripts/analyze-gemini-usage.py` counts.
#[derive(Clone, Copy)]
enum SideCallKind {
    SideCall,
    PhaseJudge,
}

/// Starts one side call on the report keys, answered with nothing when it
/// fails: logged, and never the candidate's problem. An empty interim note
/// records nothing, and an empty judgment ticks nothing; the next pause or
/// turn asks again.
fn spawn_side_call(
    kind: SideCallKind,
    prompt: String,
    interview: InterviewContext<'_>,
) -> tokio::task::JoinHandle<String> {
    let keys = Arc::clone(interview.keys);
    let model = interview.boot.report_model.to_string();
    let room_name = interview.boot.room_name.to_string();
    tokio::spawn(async move {
        let (label, result) = match kind {
            SideCallKind::SideCall => (
                "interim review",
                generate_interim_review_with_keys(&keys, &model, &prompt, &room_name).await,
            ),
            SideCallKind::PhaseJudge => (
                "phase judge",
                generate_phase_judgment_with_keys(&keys, &model, &prompt, &room_name).await,
            ),
        };
        result.unwrap_or_else(|error| {
            eprintln!(
                "{label} skipped room={room_name}: {}",
                keys.redact(&error.to_string())
            );
            String::new()
        })
    })
}

/// How long the end of the interview waits for the phase judge in all, and
/// how long a round transition is held for it. Less than the call's own
/// deadline: the candidate is waiting on the report or on the next question.
const PHASE_JUDGE_SETTLE: Duration = Duration::from_secs(5);

/// Reads the latest stretch for REACTO steps the interviewer has not recorded,
/// off to one side like the interim review. Collected on a later watch tick.
fn spawn_phase_judge(
    state: &mut RuntimeState,
    interview: InterviewContext<'_>,
) -> tokio::task::JoinHandle<String> {
    let prompt = crate::agent::take_phase_judge_window(state, interview.boot.problem);
    spawn_side_call(SideCallKind::PhaseJudge, prompt, interview)
}

/// Applies a finished judgment, logs what it recorded the way the
/// interviewer's own evidence calls are logged, and tells the interviewer.
async fn collect_phase_judgment(context: &mut GeminiEventContext<'_>, text: &str, room_name: &str) {
    let was_complete = crate::agent::coding_round_complete(context.state);
    let recorded = crate::agent::apply_phase_judgment(context.state, text);
    for evidence in &recorded {
        eprintln!(
            "evidence: at={} phase={} accepted=true source=phase_judge room={room_name}",
            log_clock(context.state),
            crate::agent::phase_id(evidence.phase),
        );
    }
    if recorded.is_empty() || context.state.ended {
        return;
    }
    let note = crate::agent::phase_judgment_note(context.state, &recorded, was_complete);
    if let Err(error) = send_model_context(
        context.gemini,
        context.state,
        ModelInputKind::Turn,
        &note,
        None,
    )
    .await
    {
        // A replacement socket's cold briefing carries the recorded steps and
        // any released follow-ups, so nothing here is owed.
        eprintln!("Gemini phase note failed ({error}); waiting for the close to be reported");
    }
}

/// Waits for one judgment until `deadline`, collecting it if it lands and
/// aborting it if it does not.
async fn await_phase_judgment(
    context: &mut GeminiEventContext<'_>,
    interview: InterviewContext<'_>,
    mut judgment: tokio::task::JoinHandle<String>,
    deadline: Instant,
) {
    match tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), &mut judgment).await {
        Ok(finished) => finish_phase_judgment(context, interview, finished).await,
        Err(_) => judgment.abort(),
    }
}

/// A judgment that has ended, however it ended: applied if it returned, and
/// otherwise a judgment that did not happen.
async fn finish_phase_judgment(
    context: &mut GeminiEventContext<'_>,
    interview: InterviewContext<'_>,
    finished: Result<String, tokio::task::JoinError>,
) {
    match finished {
        Ok(text) => collect_phase_judgment(context, &text, interview.boot.room_name).await,
        Err(error) => {
            eprintln!("phase judge ended abnormally: {error}");

            // Answered with nothing, as a failed call is, so its window is read
            // again.
            crate::agent::apply_phase_judgment(context.state, "");
        }
    }
}

/// Brings the judged steps up to date before the end, after which the report
/// is written from the evidence. The judgment in flight is waited for, then
/// one more is asked for anything said since, within one shared bound.
///
/// Only the end waits in the room loop. Waiting stops the loop forwarding the
/// candidate's audio. Its queue holds about forty seconds (see
/// `candidate_audio_stream`), so speech meanwhile is delayed rather than lost,
/// but it reaches the interviewer only after the wait; at the end nothing more
/// is owed a reply, but the round transition is held instead, see
/// `defers_round_transition`.
async fn settle_phase_judge(context: &mut GeminiEventContext<'_>, interview: InterviewContext<'_>) {
    let deadline = Instant::now() + PHASE_JUDGE_SETTLE;
    if let Some(judgment) = context.activity.phase_judge.take() {
        await_phase_judgment(context, interview, judgment, deadline).await;
    }
    if crate::agent::phase_judge_due(context.state)
        && Instant::now() < deadline
        && context.activity.reserve_phase_judge(Instant::now())
    {
        let judgment = spawn_phase_judge(context.state, interview);
        await_phase_judgment(context, interview, judgment, deadline).await;
    }
}

/// Whether a control packet is the end, which `settle_phase_judge` runs before.
fn settles_phase_judge(state: &RuntimeState, topic: &str, payload: &serde_json::Value) -> bool {
    !state.ended && control_type(topic, payload) == Some("end_interview")
}

/// The `type` of a control packet, or none for any other topic.
fn control_type<'a>(topic: &str, payload: &'a serde_json::Value) -> Option<&'a str> {
    (topic == TOPIC_CONTROL)
        .then(|| payload.get("type").and_then(serde_json::Value::as_str))
        .flatten()
}

/// Whether a round transition must wait for the phase judge. The transition
/// skips the behavioral round for an incomplete coding round and cannot be
/// taken back, so a judgment that may complete it, in flight or due for what
/// was just said, is let finish first. The packet is held rather than waited
/// on, and the watch tick applies it once the judgment is in or the hold runs
/// out; see `RuntimeActivity::parked_transition_ready`.
fn defers_round_transition(
    state: &RuntimeState,
    judge_running: bool,
    topic: &str,
    payload: &serde_json::Value,
) -> bool {
    crate::agent::round_transition_due(state, payload)
        && !state.behavioral_round_started
        && control_type(topic, payload) == Some("round_transition")
        && (judge_running || crate::agent::phase_judge_due(state))
}

/// The one side call of a kind that may be in flight, an interim review or a
/// phase judgment, owned rather than let loose.
///
/// Two things a bare `tokio::spawn` got wrong. A task that panics sends no
/// result, so a loop tracking "a review is running" in a bool would believe one
/// forever and spend a single panic to disable every remaining pause in the
/// interview; asking the handle is the same question with no second copy of the
/// answer. And `JoinHandle` detaches on drop, so an interview that ended under
/// a review left a task holding a cloned API key and writing to a log nobody
/// was reading any more. `run_room` has several exits and none of them should
/// have to remember this, so the abort is the drop.
#[derive(Default)]
struct SideCall(Option<tokio::task::JoinHandle<String>>);

impl SideCall {
    fn is_running(&self) -> bool {
        self.0.is_some()
    }

    /// The handle once it has finished, leaving the slot empty.
    ///
    /// `is_finished` is true for a task that panicked as well as one that
    /// returned, and awaiting a handle that has finished does not block, so
    /// this is the one place a review leaves the slot, however it ended.
    fn finished(&mut self) -> Option<tokio::task::JoinHandle<String>> {
        if self.0.as_ref()?.is_finished() {
            self.0.take()
        } else {
            None
        }
    }

    /// The handle, finished or not, leaving the slot empty.
    fn take(&mut self) -> Option<tokio::task::JoinHandle<String>> {
        self.0.take()
    }

    fn start(&mut self, handle: tokio::task::JoinHandle<String>) {
        if let Some(replaced) = self.0.replace(handle) {
            replaced.abort();
        }
    }
}

impl Drop for SideCall {
    fn drop(&mut self) {
        if let Some(handle) = self.0.take() {
            handle.abort();
        }
    }
}

/// The stretch to review, with the cursor moved past it.
///
/// Split from the spawn above so the part that can be wrong is reachable from a
/// test. Which lines a pause reads, and whether the pause after it reads the
/// ones since rather than the same ones again, is the whole behavior here; the
/// rest is an HTTP call.
///
/// The window is marked read here, before the call goes out, rather than when
/// one returns. A call that fails would otherwise hand the same stretch to the
/// next pause, which would then be reading old speech instead of the speech
/// since -- and the transcript reaches the final reviewer whole either way, so
/// a window nobody managed to summarize is not a window anybody lost.
fn take_interim_review_window(state: &mut RuntimeState, boot: &RuntimeBootstrap<'_>) -> String {
    // Both halves bounded. The cursor only moves when a review actually fires,
    // so a stretch that never offers a quiet moment banks lines indefinitely,
    // and the editor is unbounded on the way in -- either one would otherwise
    // hand a call that has twelve seconds an input too large to read.
    let window = transcript_tail(
        &crate::agent::mark_unrecognized_turns(
            &state.delivery_transcript()[unreviewed_from(state)..],
        ),
        INTERIM_WINDOW_BYTES,
    );
    state.interim_transcript_lines = state.transcript.len();

    // The tail, not the whole list. Every review used to be sent every note
    // taken so far, which is prefill growing quadratically across a session to
    // defend against a repeat `record_interim_notes` already drops.
    let recent = state
        .interim_notes
        .len()
        .saturating_sub(INTERIM_CONTEXT_NOTES);
    let evidence = state
        .prompt_evidence(crate::agent::ViewFor::Interim)
        .join("\n");
    let code = if state.code.trim().is_empty() {
        // The cursor moves here too, because it records what the last review
        // showed the model rather than the last code it was sent. Left behind,
        // a candidate who clears the editor and then restores exactly what was
        // there before is reported as unchanged, to a model whose last look at
        // the editor found it empty.
        state.interim_code = state.code.clone();
        String::new()
    } else if state.code == state.interim_code {
        INTERIM_CODE_UNCHANGED.to_string()
    } else {
        state.interim_code = state.code.clone();
        code_head(&state.code, INTERIM_CODE_BYTES)
    };
    let prompt = interim_review_prompt(&InterimReviewInput {
        problem: boot.problem,
        interview_mode: state.interview_mode,
        example_drawing_enabled: state.example_drawing_enabled,
        transcript_window: &window,
        code: &code,
        language: &state.language,
        already_recorded: &state.interim_notes[recent..].join("\n"),
        evidence: &evidence,
    });
    state.evidence_ledger.record_model_input(
        ModelInputKind::Interim,
        &format!("{}\n\n{prompt}", crate::agent::interim_system_instruction()),
    );
    prompt
}

/// Everything the interview loop needs, owned, once the candidate has joined
/// and both sides are talking.
///
/// Owned and not a set of borrows: the loop builds `InterviewContext` and
/// `RoomIdentities` out of these fields, and a struct that held those views
/// alongside the values they point at would be borrowing itself. `'a` is the
/// caller's, carried only by `boot`.
struct OpenSession<'a> {
    room: RoomLink,
    events: tokio::sync::Mutex<RoomEventReader>,
    agent_identity: String,
    candidate_identity: String,
    boot: RuntimeBootstrap<'a>,
    output_audio: OutputAudio,
    gemini: crate::gemini::GeminiLiveSession,
    media: CandidateMedia,
    turn: TurnState,
    started_at: Instant,
    restarts: usize,
    deferred_events: std::collections::VecDeque<TimedRoomEvent>,
}

#[derive(Debug)]
struct StartupDeliveryError(Box<dyn std::error::Error + Send + Sync>);

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

// A failed ready notice can follow a paid generation. Stop the reader before
// draining so the common startup summary receives each observation once.
async fn publish_startup_readiness(
    gemini: &mut GeminiLiveSession,
    usage: &mut crate::gemini::TokenUsage,
    events: &tokio::sync::Mutex<RoomEventReader>,
    deferred: &mut Vec<TimedRoomEvent>,
    room_closed: &mut bool,
    publication: impl std::future::Future<Output = Result<(), Box<dyn std::error::Error + Send + Sync>>>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Keep observation separate from replay: a timely return must survive a
    // slow publish, and the first runtime watch may run before its event arm.
    // No handlers, media writes or reconnect notices run inside this budget.
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
                    if deferred.len() >= STARTUP_READY_EVENT_LIMIT {
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
        let _ = gemini.shutdown().await;
        for observed in gemini.drain_usage() {
            usage.add(observed);
        }
    }
    result.map_err(|error| StartupDeliveryError(error).into())
}

// Publication only queues the notice. Keep its sender in the roster until the
// candidate acknowledges it, within the same budget and without retransmission.
async fn publish_startup_failure(
    publication: impl std::future::Future<Output = Result<(), Box<dyn std::error::Error + Send + Sync>>>,
    receipt: impl std::future::Future<Output = ()>,
) {
    let _ = tokio::time::timeout(RECONNECT_NOTICE_LIMIT, async {
        if publication.await.is_ok() {
            receipt.await;
        }
    })
    .await;
}

fn is_startup_receipt(
    topic: Option<&str>,
    sender: Option<&str>,
    candidate: &str,
    payload: &[u8],
    id: &str,
) -> bool {
    topic == Some(TOPIC_CONTROL)
        && is_interview_participant(sender, candidate)
        && serde_json::from_slice::<serde_json::Value>(payload)
            .is_ok_and(|value| value["type"] == "startup_received" && value["deliveryId"] == id)
}

async fn account_startup<T>(
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
        log_startup_failure(live_usage_line(
            room_name,
            session_id,
            &config.gemini_live_model,
            0,
            LiveOutcome::from_error(error.as_ref(), LiveOutcome::Error),
            None,
            &startup_usage,
        ));
    }
    opened
}

enum OpenSessionOutcome<'a> {
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
async fn open_session<'a>(
    config: &'a AgentConfig,
    keys: &Arc<GeminiKeys>,
    room_name: &'a str,
    now_seconds: u64,
    session_id: u64,
    log_startup_failure: impl FnOnce(String),
) -> Result<OpenSessionOutcome<'a>, Box<dyn std::error::Error + Send + Sync>> {
    let agent_identity = agent_identity(room_name);
    let joined = async {
        let (room, mut events) = join_room(config, room_name, &agent_identity, now_seconds).await?;
        let room = RoomLink::live(room);
        let mut agent_state = session::AgentState::default();
        session::publish_opening_attributes(&room, &mut agent_state, config.gemini_silence_ms)
            .await?;

        // The candidate's metadata picks the problem, so nothing else can start
        // until someone joins.
        let mut deferred_events = Vec::new();
        let candidate =
            candidate_metadata(room.connected(), &mut events, &mut deferred_events).await?;
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
    let Some((candidate_identity, candidate_metadata)) = candidate else {
        eprintln!("no candidate joined room={room_name}; leaving it");
        return Ok(OpenSessionOutcome::NoCandidate);
    };
    account_startup(config, room_name, session_id, log_startup_failure, async |startup_usage| {
    let events = tokio::sync::Mutex::new(RoomEventReader::from(events));
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
        let (keys, slot) = (keys.clone(), tool_ack.clone());
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
    let mut room_closed = false;
    let prepared = async {
        let mut room_ready = Duration::ZERO;
        let mut gemini_ready = Duration::ZERO;
        let (mut output_audio, mut gemini) = tokio::try_join!(
            async {
                isolate_local_agent(config, room_name, &agent_identity, now_seconds).await?;
                let published = publish_output_audio(room.connected(), GEMINI_OUTPUT_AUDIO_SAMPLE_RATE).await;
                room_ready = setup_began.elapsed();
                published
            },
            async {
                let interview = InterviewContext {
                    config,
                    keys,
                    boot: &boot,
                    started_at: setup_began,
                    candidate_identity: &candidate_identity,
                    events: &events,
                    slot: None,
                };
                let opened = crate::gemini::first_open_within(
                    crate::gemini::FIRST_OPEN_LIMIT,
                    open_cold_session(interview, &mut restarts, None),
                )
                .await;
                gemini_ready = setup_began.elapsed();
                opened
            },
        )?;
        output_audio.tool_ack = tool_ack;
        output_audio.tool_ack_task = tool_ack_task;
        eprintln!(
            "timing: room and Gemini session ready {:.2}s after the candidate joined (room {:.2}s, gemini {:.2}s)",
            setup_began.elapsed().as_secs_f64(),
            room_ready.as_secs_f64(),
            gemini_ready.as_secs_f64()
        );

        // Only prompt construction uses this provisional clock. The opening
        // starts with its full budget, and no watch or round transition runs
        // before ready.
        let started_at = Instant::now();

        let mut turn = TurnState {
            state: initial_runtime_state(&boot, started_at),
            agent_state: std::mem::take(&mut agent_state),
            activity: runtime_activity(config, started_at, session_id),
            turns: SpeakerTurns::default(),
            transcripts: Default::default(),
        };
        turn.state
            .evidence_ledger
            .record_model_input(ModelInputKind::LiveSetup, &boot.instructions);
        let mut media = CandidateMedia::new();

        // Keep subscriptions consumed while waiting for candidate metadata. The
        // startup wait pumps them too; dropping a subscription here would leave
        // both waits deaf for the whole interview.
        for (received_at, event) in std::mem::take(&mut deferred_events) {
            if !handle_media_event(
                &mut media,
                &mut gemini,
                &candidate_identity,
                boot.candidate_video,
                &event,
            )
            .await? {
                deferred_events.push((received_at, event));
            }
        }

        let greeting = with_timer(&turn.state, boot.greeting.clone());
        let opening_requested_at = Instant::now();
        let started_at = match probe_live_output(
            &mut gemini,
            &mut turn.state,
            &greeting,
            startup_output_limit(config),
            &mut StartupContext {
                media: &mut media,
                candidate_identity: &candidate_identity,
                candidate_video: boot.candidate_video,
                deferred_events: &mut deferred_events,
                room_closed: &mut room_closed,
            },
            &mut &events,
        )
        .await
        {
            Ok(started_at) => started_at,
            Err(error) => {
                if !room_closed {
                    gemini.record_failure(keys);
                }
                let _ = gemini.shutdown().await;

                // A failed generation can still be billed. Only observed
                // counters are reported; absence of observations is not a claim
                // of zero cost.
                for usage in gemini.drain_usage() {
                    startup_usage.add(usage);
                }
                return Err(error);
            }
        };
        turn.state.started_at = started_at;
        turn.activity = ready_runtime_activity(
            config,
            room_name,
            started_at,
            opening_requested_at,
            session_id,
            &greeting,
        );
        let ready = startup_ready_notice(started_at, turn.activity.startup_ready_at);
        publish_startup_readiness(
            &mut gemini,
            startup_usage,
            &events,
            &mut deferred_events,
            &mut room_closed,
            publish_interviewer_state(&room, &ready),
        )
        .await?;
        Ok::<_, Box<dyn std::error::Error + Send + Sync>>((
            output_audio,
            gemini,
            media,
            turn,
            started_at,
        ))
    }
    .await;

    let (output_audio, gemini, media, turn, started_at) = match prepared {
        Ok(prepared) => prepared,
        Err(error) => {
            if !room_closed && let Ok(packet) = browser_packet(TOPIC_CONTROL, &startup_state(Some(error.as_ref()))) {
                let id = crate::sha256_hex(&[&packet.payload]);
                publish_startup_failure(
                    room.publish_data(packet),
                    async {
                        let mut events = events.lock().await;
                        while let Some(event) = events.recv().await {
                            match event {
                                RoomEvent::DataReceived { payload, topic, participant, .. } => {
                                    let sender = participant.as_ref().map(|participant| participant.identity().0);
                                    if is_startup_receipt(topic.as_deref(), sender.as_deref(), &candidate_identity, &payload, &id) {
                                        return;
                                    }
                                }
                                RoomEvent::Disconnected { .. } => return,
                                RoomEvent::ParticipantDisconnected(participant) if participant.identity().0 == candidate_identity => return,
                                _ => {}
                            }
                        }
                    },
                ).await;
            }
            leave_room(&room).await;
            return Err(error);
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
        deferred_events: deferred_events.into(),
    })))
    }).await
}

/// The room loop's own bookkeeping, distinct from the session it runs on.
///
/// Four things only the loop reads, bundled so an arm that needs three of them
/// takes one parameter rather than three.
struct RoomLoop {
    /// How many sockets this interview has been through.
    restarts: usize,
    deferred_restart: DeferredRestart,
    resume_recovery: ResumeRecovery,

    /// At most one idle-window review at a time, collected on the watch tick.
    /// A tick of latency on a note nobody is waiting for is not worth an arm
    /// in the select.
    interim_review: SideCall,

    /// Checked on the watch tick rather than given its own timer arm: the tick
    /// already runs, and a resolution of one tick is plenty for a ninety-second
    /// grace.
    presence: CandidatePresence,
}

/// The interview reached the deadline the server holds for it.
async fn on_hard_deadline(
    room: &RoomLink,
    context: &mut GeminiEventContext<'_>,
    loops: &mut RoomLoop,
    interview: InterviewContext<'_>,
) -> Result<ControlFlow<()>, Box<dyn std::error::Error + Send + Sync>> {
    eprintln!(
        "interview reached its server-side deadline: room={} duration={}min",
        interview.boot.room_name, interview.boot.duration_min
    );

    end_through_control(
        room,
        context,
        interview,
        "time_up",
        &mut loops.interim_review,
    )
    .await?;
    // `end_through_control` has already published the report and left the room.
    Ok(ControlFlow::Break(()))
}

/// Reconcile compressed context as soon as usage and output boundaries permit.
/// The watch tick retries when candidate speech or queued output holds it.
async fn maybe_refresh_context(room: &RoomLink, context: &mut GeminiEventContext<'_>) {
    // Runs after every Gemini event, most of them audio. Nothing pending is the
    // common case, and the gates below read the candidate's open turn to say
    // no, so it is answered first.
    if !context.activity.tokens.context_refresh_pending {
        return;
    }
    if context.activity.checkpoint_due(
        context.state,
        context.output_audio.is_playing(),
        context.turns.candidate.is_open(),
        Instant::now(),
    ) {
        let checkpoint = with_timer(
            context.state,
            crate::agent::compressed_context(context.state),
        );
        let input_before = context.state.evidence_ledger.metrics.turn_prompt_bytes;
        match send_model_context_scoped(
            context.gemini,
            context.state,
            ModelInputKind::Turn,
            &checkpoint,
            None,
            ContinuityScope::ColdOrCheckpoint,
        )
        .await
        {
            Ok(()) => {
                context.activity.tokens.context_refresh_pending = false;
                context.activity.sync_cut_plausible(context.state);
                eprintln!(
                    "codetrial context_refresh room={} session={} bytes={}",
                    room.name(),
                    context.activity.live_session_id,
                    context.state.evidence_ledger.metrics.turn_prompt_bytes - input_before
                );
            }
            Err(error) => eprintln!(
                "Gemini context refresh failed ({error}); waiting for the close to be reported"
            ),
        }
    }
}

/// One watch tick: the candidate's absence, the interim review, and the nudge.
async fn on_watch_tick(
    room: &RoomLink,
    context: &mut GeminiEventContext<'_>,
    loops: &mut RoomLoop,
    interview: InterviewContext<'_>,
    tick_at: Instant,
) -> Result<ControlFlow<()>, Box<dyn std::error::Error + Send + Sync>> {
    // One reading of the clock for the whole tick. Three calls gave three
    // instants microseconds apart, so a cooldown stamped from one and tested
    // against another was answering a question nobody asked. Judge pending
    // presence without waiting for unrelated queued work. A timely return may
    // already be queued, while a long packet handler must not postpone an
    // absence that has actually expired.
    let events = interview.events.lock().await;
    let presence =
        loops.presence.projected(
            events
                .deferred
                .iter()
                .filter_map(|(at, event)| match event {
                    RoomEvent::ParticipantDisconnected(participant)
                        if participant.identity().0 == interview.candidate_identity =>
                    {
                        Some(CandidatePresenceChange::Left(*at))
                    }
                    RoomEvent::ParticipantConnected(participant)
                        if participant.identity().0 == interview.candidate_identity =>
                    {
                        Some(CandidatePresenceChange::Returned(*at))
                    }
                    _ => None,
                }),
        );
    drop(events);
    if presence.gave_up(tick_at) {
        // No report: it would be graded from a session the candidate walked out
        // of, and there is nobody in the room to receive it. The browser writes
        // the report the candidate actually sees.
        eprintln!(
            "candidate did not return to room={}; ending",
            interview.boot.room_name
        );

        // `shutdown` rather than `close`: it is the same teardown for a caller
        // holding a borrow, which is what every handler here has. Not `?`: a
        // socket that fails its close is no reason to stay in the room.
        if let Err(error) = context.gemini.shutdown().await {
            eprintln!("Gemini close failed ({error}); leaving anyway");
        }
        leave_room(room).await;
        return Ok(ControlFlow::Break(()));
    }

    // A board the send interval held back goes out here once nothing stops it.
    // Without this it waited for the next board to carry it, and the candidate
    // who has stopped drawing to explain is exactly the one who sends no next
    // board.
    if let Err(error) = board::send_if_due(context.board, context.gemini, tick_at).await {
        eprintln!("Gemini board write failed ({error}); waiting for the close to be reported");
    }

    // A prompt Gemini never answered holds the floor, and a held floor keeps
    // both the silence nudge and a deferred `GoAway` waiting on output that is
    // not coming. The reply stays owed, so a replacement socket still gives it.
    // After the absence check, so a room being torn down is not reconnected.
    let stalls = context
        .activity
        .settle_stalls(tick_at, context.output_audio.is_playing());
    if stalls.prompt_released {
        eprintln!(
            "timing: no output {}s after a prompt; returning the floor at={} room={}",
            PROMPT_STALL.as_secs(),
            log_clock(context.state),
            interview.boot.room_name
        );
    }

    // A close waits on its tool acknowledgement, and the check after each
    // Gemini event is what normally sees it through. A socket that never sends
    // the acknowledgement sends no event either; the stall above releases the
    // hold, and this is the only place left to act on it. Ahead of the reply
    // watch, which stays out of a requested close, and of the nudges it holds
    // back, which were what used to provoke the event.
    if ready_to_close(context.state, context.activity) {
        eprintln!(
            "interviewer ended the interview: trigger=stalled_acknowledgement room={}",
            interview.boot.room_name
        );
        end_through_control(
            room,
            context,
            interview,
            "interview_complete",
            &mut loops.interim_review,
        )
        .await?;
        return Ok(ControlFlow::Break(()));
    }
    if !context.state.interviewer_silenced()
        && !context.state.end_requested
        && context.activity.handoff_reply_due(
            tick_at,
            Duration::from_millis(u64::from(interview.boot.silence_ms)),
            context.output_audio.is_playing(),
        )
    {
        context.activity.drop_handoff_reply();
        match session::request_handoff_reply(context.gemini, context.state).await {
            Ok(()) => {
                context.activity.renew_handoff_prompt(tick_at);
                eprintln!(
                    "handoff reply requested: the candidate went quiet after the handoff at={} room={}",
                    log_clock(context.state),
                    interview.boot.room_name
                );
            }
            Err(error) => eprintln!(
                "handoff reply request failed ({error}); waiting for the reply timeout room={}",
                interview.boot.room_name
            ),
        }
    }
    if reply_watch(
        context.state,
        context.activity,
        tick_at,
        context.output_audio.is_playing(),
    ) == ReplyWatch::Recover
    {
        loops.deferred_restart.cancel();
        let waited = context
            .activity
            .expired_reply_wait(tick_at, context.output_audio.is_playing())
            .unwrap_or(context.activity.reply_timeout);
        eprintln!(
            "Gemini reply timed out after {}s; reconnecting at={} room={}",
            waited.as_secs(),
            log_clock(context.state),
            interview.boot.room_name
        );
        return replace_gemini_session(room, context, interview, loops, Trigger::Watchdog, None)
            .await;
    }
    if loops
        .deferred_restart
        .watch_due(context.activity, stalls.spend_restart, tick_at)
        && spend_deferred_restart(room, context, loops, interview, "stall")
            .await?
            .is_break()
    {
        return Ok(ControlFlow::Break(()));
    }

    // Read again rather than reused from above: the held `GoAway` just spent
    // may have replaced the socket, and a cold replacement always owes the
    // reply to its briefing.
    let reply = reply_watch(
        context.state,
        context.activity,
        tick_at,
        context.output_audio.is_playing(),
    );

    // Every path that settles the wait already publishes the next state: audio
    // publishes `speaking`, a completed or cut turn, a replacement and a hold
    // publish `listening`, and so does the candidate speaking again.
    if reply == (ReplyWatch::Owed { shown: true }) {
        set_agent_state(room, context.agent_state, AGENT_STATE_THINKING);
    }

    // Not `?`, for the reason the nudge below gives. A ping that times out has
    // already ended the reader, so the close it found is reported next.
    if let Err(error) = context.gemini.keep_alive().await {
        eprintln!("Gemini ping failed ({error}); waiting for the close to be reported");
    }

    if let Some(judgment) = context.activity.phase_judge.finished() {
        finish_phase_judgment(context, interview, judgment.await).await;
    }
    if context
        .activity
        .held_transition_needs_judgment(context.state, tick_at)
        && context.activity.reserve_phase_judge(tick_at)
    {
        let judgment = spawn_phase_judge(context.state, interview);
        context.activity.phase_judge.start(judgment);
    }
    if context
        .activity
        .parked_transition_ready(context.state, tick_at)
        && let Some(parked) = context.activity.pending_round_transition.take()
        && apply_data_packet(
            room,
            context,
            interview,
            TOPIC_CONTROL,
            &parked.payload,
            parked.received,
        )
        .await?
        .is_break()
    {
        return Ok(ControlFlow::Break(()));
    }

    // A judgment collected above, a page that rejoined, or a publish that
    // failed: the tick is where all three reach the page.
    session::flush_framework_progress(room, context.state).await;
    if !context.activity.phase_judge.is_running()
        && context.activity.claim_phase_judge(context.state, tick_at)
    {
        let judge = spawn_phase_judge(context.state, interview);
        context.activity.phase_judge.start(judge);
    }

    if let Some(review) = loops.interim_review.finished() {
        match review.await {
            Ok(notes) => record_interim_notes(context.state, &notes),

            // A panicked or cancelled review is a review that did not happen.
            // The slot is already clear, so the next pause takes it.
            Err(error) => {
                eprintln!("interim review ended abnormally: {error}");
            }
        }
    }

    // Ahead of the nudge below, and cheap when it declines. The pause this
    // reads is the same pause the interview is spending anyway, and what it
    // buys is a final reviewer that arrives at a session somebody has already
    // read.
    //
    // Not a shorter wait: the report call is bounded at REPORT_TIMEOUT and
    // measures seconds, and the transcript still reaches it whole, so this is
    // bought for what the report is written from rather than for when it lands.
    // The ten minutes in issue 31 were the interview's own clock, and it is
    // `end_interview` that answers those.
    if !loops.interim_review.is_running()
        && context
            .activity
            .claim_interim_review(context.state, tick_at)
    {
        loops
            .interim_review
            .start(spawn_interim_review(context.state, interview));
    }
    context
        .activity
        .settle_stale_request(context.state, tick_at, crate::current_epoch_millis());
    if let Some(prompt) = context
        .activity
        .claim_thinking_reply_fallback(context.state, tick_at)
    {
        eprintln!(
            "thinking: no reply {}s after the hold ended; asking for it room={}",
            THINKING_REPLY_FALLBACK.as_secs(),
            interview.boot.room_name
        );
        // Owed either way: a replacement socket gives it instead.
        if let Err(error) = send_model_text(
            context.gemini,
            context.state,
            ModelInputKind::Turn,
            TurnCause::Turn,
            &prompt,
        )
        .await
        {
            eprintln!(
                "Gemini thinking reply failed ({error}); waiting for the close to be reported"
            );
        }
        context
            .activity
            .mark_prompted(tick_at, Some(&prompt), false);
    }
    if context
        .state
        .claim_thinking_check_in(tick_at, crate::current_epoch_millis())
    {
        let prompt = crate::agent::with_timer(
            context.state,
            crate::agent::thinking_check_in(context.state),
        );
        match send_model_text(
            context.gemini,
            context.state,
            ModelInputKind::Watch,
            TurnCause::Watch,
            &prompt,
        )
        .await
        {
            Ok(()) => context.state.clear_thinking_debt(),
            Err(error) => {
                eprintln!("Gemini check-in failed ({error}); waiting for the close to be reported");
            }
        }
        // Owed either way: a replacement socket gives the check-in instead.
        context
            .activity
            .mark_prompted(tick_at, Some(&prompt), false);
        eprintln!(
            "{}",
            prompt_line(
                context.state,
                context.activity,
                "kind=thinking_check_in",
                interview.boot.room_name,
            )
        );
        return Ok(ControlFlow::Continue(()));
    }
    maybe_refresh_context(room, context).await;

    if let Some(body) = context
        .activity
        .pending_silent_editor(context.state, tick_at)
        && let Err(error) =
            send_silent_editor(context.gemini, context.state, context.activity, &body).await
    {
        eprintln!("silent editor context failed ({error}); waiting for the close to be reported");
    }

    // A nudge cannot repair an unanswered turn and would replace the prompt
    // debt (and its timer) before the watchdog could recover it.
    if reply != ReplyWatch::Settled {
        return Ok(ControlFlow::Continue(()));
    }
    if let Some(prompt) = context.activity.watch_prompt(context.state, tick_at) {
        // Not `?`. Every write below is one the reader may be about to explain:
        // a socket Gemini has closed fails the next send long before
        // `next_event` drains and reports it, and audio flushes every hundred
        // milliseconds, so propagating here is how a closed socket ended the
        // interview from the write side without the arm that resumes it ever
        // running.
        if let Err(error) = send_watched_prompt(
            context.activity,
            &prompt,
            send_model_text(
                context.gemini,
                context.state,
                ModelInputKind::Watch,
                TurnCause::Watch,
                &prompt.text,
            ),
        )
        .await
        {
            eprintln!("Gemini nudge failed ({error}); waiting for the close to be reported");
            context.activity.unsend_watch_prompt(context.state);
            return Ok(ControlFlow::Continue(()));
        }
        let kind = if prompt.allows_silence {
            "kind=review"
        } else {
            "kind=nudge"
        };
        eprintln!(
            "{}",
            prompt_line(
                context.state,
                context.activity,
                kind,
                interview.boot.room_name,
            )
        );
    }

    Ok(ControlFlow::Continue(()))
}

fn completed_live_reply(activity: &RuntimeActivity, event: &GeminiEvent) -> bool {
    matches!(event, GeminiEvent::TurnComplete)
        && activity.generating
        && !activity.socket.discarding_output
}

/// What a watch tick does about the reply the interviewer owes, decided in one
/// place so the order the tick acts in is the order a test can drive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ReplyWatch {
    /// Nothing is owed or under way, so the silence nudges may run.
    Settled,
    /// A reply is owed or under way, and not yet overdue. `shown` once it has
    /// waited long
    /// enough that the room should say the interviewer is working on it. No
    /// nudge either way: one would replace the debt and its timer.
    Owed { shown: bool },
    /// Owed past the reply timeout: replace the socket.
    Recover,
}

// VAD edges are not acoustic evidence. Keep this on the same path the room uses
// to pump each frame.
/// Returns whether the frame carried voice, which the camera gate reads on
/// every frame whatever the idle timers make of it.
fn note_candidate_audio_frame(
    state: &RuntimeState,
    activity: &mut RuntimeActivity,
    frame: Option<&::livekit::webrtc::audio_frame::AudioFrame<'_>>,
    now: Instant,
) -> bool {
    let voiced = frame.is_some_and(media::frame_has_voice);
    if voiced && !state.interviewer_silenced() {
        activity.turn_timing.note_voice(now);
        activity.latency.voice(now);
    }
    if voiced
        && (state.context_compression.is_some()
            || !activity.transcribed_on_socket
            || activity.candidate_voice_active)
    {
        activity.note_candidate_voice(now);
    }
    voiced
}

/// While the floor is held by the candidate keeping it to think, nothing is
/// waited on and nothing is shown: the interviewer's silence
/// is the one the candidate asked for, and replies generated meanwhile are
/// dropped on purpose. Once a close is requested the silence is the one the
/// tool response asked for, so neither recovery nor the wait applies, but the
/// nudges still stay out of it.
fn reply_watch(
    state: &RuntimeState,
    activity: &RuntimeActivity,
    now: Instant,
    audio_playing: bool,
) -> ReplyWatch {
    // Checked first: a reply that can time out is always unfinished.
    if !activity.reply_unfinished() {
        return ReplyWatch::Settled;
    }
    let watched = !state.interviewer_silenced() && !state.end_requested;
    if watched && activity.reply_timed_out(now, audio_playing) {
        return ReplyWatch::Recover;
    }
    ReplyWatch::Owed {
        shown: watched && activity.reply_visibly_late(now, audio_playing),
    }
}

/// Marks the prompt owed once its send succeeded. A failed send is the
/// caller's to account for.
fn record_event_prompt<E>(
    activity: &mut RuntimeActivity,
    prompt: &str,
    sent: Result<(), E>,
    now: Instant,
) -> Result<(), E> {
    sent?;
    activity.mark_prompted(now, Some(prompt), false);
    Ok(())
}

// This appends history without requesting output. Wire order does not promise
// consumption order against realtime audio already sent to the provider.
async fn send_silent_editor(
    gemini: &mut GeminiLiveSession,
    state: &mut RuntimeState,
    activity: &mut RuntimeActivity,
    body: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    session::send_plain_context(gemini, state, body).await?;
    activity.commit_silent_editor(state);

    // The model now holds this code, so later updates diff from it rather than
    // sending it again. Review is governed by the revision the watcher last
    // reviewed, not by this baseline, so the change stays owed its review.
    state.code_shown = state.code.clone();
    eprintln!(
        "codetrial silent_editor_context local_write=true at_ms={} revision={} language={} source_sha256={} socket={}",
        crate::agent::elapsed_ms(state),
        state.evidence_ledger.code.substantive_revision,
        state.language,
        state.evidence_ledger.code.latest_digest,
        activity.live_socket
    );
    Ok(())
}

/// Failed writes leave the behavioral invitation available after reconnect.
/// Cooldowns still bound retries while the old socket reports its close.
async fn send_watched_prompt<E>(
    activity: &mut RuntimeActivity,
    prompt: &WatchPrompt,
    send: impl std::future::Future<Output = Result<(), E>>,
) -> Result<(), E> {
    send.await?;
    activity.commit_watch_prompt(prompt, Instant::now());
    if prompt.behavioral_nudge {
        activity.behavioral_nudged = true;
    }
    activity.mark_prompted(Instant::now(), Some(&prompt.text), prompt.allows_silence);
    Ok(())
}

/// One Gemini event, plus the two restarts only this arm can decide.
async fn on_gemini_event(
    room: &RoomLink,
    context: &mut GeminiEventContext<'_>,
    event: Option<GeminiEvent>,
    loops: &mut RoomLoop,
    interview: InterviewContext<'_>,
) -> Result<ControlFlow<()>, Box<dyn std::error::Error + Send + Sync>> {
    let Some(event) = event else {
        // Gemini hung up. Usually that is the ten-minute cap on a single
        // connection rather than anything wrong, so the session continues on a
        // new socket instead of ending the interview. The close itself performs
        // the replacement, so any advisory still held is already paid for.
        //
        // Held means the replacement it asked for never happened in time: the
        // server closed the socket, and the resume starts from whatever
        // checkpoint it last offered. That is the outcome the deferral exists
        // to avoid, so it is logged by name.
        if loops.deferred_restart.is_armed() {
            eprintln!(
                "goaway expired: the server closed the socket before the output settled; at={} room={}",
                log_clock(context.state),
                interview.boot.room_name
            );
        }
        loops.deferred_restart.cancel();
        if replace_gemini_session(room, context, interview, loops, Trigger::Close, None)
            .await?
            .is_break()
        {
            return Ok(ControlFlow::Break(()));
        }
        return Ok(ControlFlow::Continue(()));
    };

    // Completed output proves recovery worked, unless it answered the recovery
    // briefing; merely staying connected does not clear consecutive watchdog
    // failures.
    loops
        .resume_recovery
        .note_event(&mut loops.restarts, context.state, context.activity, &event);

    if let GeminiEvent::GoAway { time_left } = &event {
        eprintln!(
            "Gemini requested a transport restart in {time_left}; room={}",
            interview.boot.room_name
        );

        // A turn still generating can be carrying a tool call or an audio
        // fragment that has not reached the room. Generated audio is also still
        // live work: replace_gemini_session clears its queue, so wait for the
        // playout arm to drain it.
        if !loops.deferred_restart.request(
            context.activity.floor,
            context.output_audio.is_playing(),
            context.activity.reply_in_flight()
                || context.activity.candidate_voice_active
                || context
                    .activity
                    .candidate_transcription_pending(Instant::now()),
            context.activity.socket.tool_response_outstanding,
        ) {
            eprintln!(
                "goaway held: at={} floor={:?} playing={} reply_pending={} tool_owed={} room={}",
                log_clock(context.state),
                context.activity.floor,
                context.output_audio.is_playing(),
                context.activity.reply_in_flight(),
                context.activity.socket.tool_response_outstanding,
                interview.boot.room_name
            );
            return Ok(ControlFlow::Continue(()));
        }
        if replace_gemini_session(room, context, interview, loops, Trigger::GoAway, None)
            .await?
            .is_break()
        {
            return Ok(ControlFlow::Break(()));
        }
        return Ok(ControlFlow::Continue(()));
    }
    handle_gemini_event(room, context, event, Interruptible::Yes).await?;
    maybe_refresh_context(room, context).await;

    // Jim called `end_interview`. Fed through the same packet the browser and
    // the server-side deadline both send, for the reason the deadline arm
    // gives: the wrap-up, the report and the teardown are then the ones that
    // are already tested.
    //
    // Read here rather than inside the tool call because this is where the
    // room, the report and the way out of the loop are all in scope.
    //
    // Held until the tool response's own generation has landed. Gemini owes one
    // for every tool response, so acting on the flag the instant it is set
    // means the closing is requested while that acknowledgement is still
    // coming: it is the acknowledgement's `TurnComplete` that settles the
    // output, `send_wrap_up_and_wait` returns on it, and the closing turn --
    // the thanks the candidate hears and the `skipped` evidence calls the
    // wrap-up asks for -- is cut off by the shutdown behind it. `TurnComplete`,
    // `Interrupted` and a socket replacement all clear the flag, so this is a
    // beat, not a condition that can hold the interview open. Not during a
    // thinking hold either: the closing would be generated into output this
    // loop drops (`output_disposition`), so the candidate hears none of it and
    // is handed a report out of a silence they did not know had ended. Held
    // instead until the hold ends; if it never does, the deadline still ends
    // the interview.
    if ready_to_close(context.state, context.activity) {
        eprintln!(
            "interviewer ended the interview: trigger=acknowledged room={}",
            interview.boot.room_name
        );
        end_through_control(
            room,
            context,
            interview,
            "interview_complete",
            &mut loops.interim_review,
        )
        .await?;
        return Ok(ControlFlow::Break(()));
    }

    // Asked after every event, not only after a turn boundary. A candidate
    // talking over the draining queue empties it through `drop_stale_playout`
    // on an `InputTranscript`, and Gemini sends no `Interrupted` for a turn it
    // already considers finished, so a boundary-only check waits out the whole
    // `timeLeft` and lets the socket drop cold instead.
    if spend_deferred_restart(room, context, loops, interview, "event")
        .await?
        .is_break()
    {
        return Ok(ControlFlow::Break(()));
    }

    Ok(ControlFlow::Continue(()))
}

/// Spends a held `GoAway` if the floor has settled, and names what let it go.
/// Every place that hands the floor back asks this.
async fn spend_deferred_restart(
    room: &RoomLink,
    context: &mut GeminiEventContext<'_>,
    loops: &mut RoomLoop,
    interview: InterviewContext<'_>,
    trigger: &str,
) -> Result<ControlFlow<()>, Box<dyn std::error::Error + Send + Sync>> {
    // Event and playout callers reach here without the watch-tick gate. The
    // handoff is judged first: the speech guard on an ordinary spend refuses
    // the very utterance it finalizes.
    if loops
        .deferred_restart
        .handoff_due(context.activity, Instant::now())
        && output_settled(context.activity.floor, context.output_audio.is_playing())
        && !context.activity.socket.tool_response_outstanding
    {
        let handoff = session::finalize_speech_handoff(
            context.gemini,
            context.state,
            &mut context.media.audio_bytes,
        )
        .await;
        loops.deferred_restart.cancel();
        let handoff = match handoff {
            Ok(handoff) => {
                for text in &handoff.transcript_fragments {
                    handle_gemini_event(
                        room,
                        context,
                        GeminiEvent::InputTranscript(text.clone()),
                        Interruptible::Yes,
                    )
                    .await?;
                }
                eprintln!(
                    "speech handoff: prefix_fragments={} discarded_events={} at={} room={}",
                    handoff.transcript_fragments.len(),
                    handoff.discarded,
                    log_clock(context.state),
                    interview.boot.room_name
                );
                Some(handoff)
            }
            Err(error) => {
                eprintln!(
                    "speech handoff failed ({error}); recovering transport room={}",
                    interview.boot.room_name
                );
                None
            }
        };
        return replace_gemini_session(room, context, interview, loops, Trigger::GoAway, handoff)
            .await;
    }
    if !loops
        .deferred_restart
        .can_spend(context.activity, Instant::now())
    {
        return Ok(ControlFlow::Continue(()));
    }
    if !loops.deferred_restart.take_if_settled(
        context.activity,
        context.output_audio.is_playing(),
        Instant::now(),
    ) {
        return Ok(ControlFlow::Continue(()));
    }
    eprintln!(
        "goaway spent: trigger={trigger} at={} room={}",
        log_clock(context.state),
        interview.boot.room_name
    );
    replace_gemini_session(room, context, interview, loops, Trigger::GoAway, None).await
}

/// Wake for a queued playout deadline even when it has already elapsed.
async fn wait_for_playout(floor: Floor, deadline: Instant) {
    if floor == Floor::AwaitingPlayout {
        // A busy loop can first poll this after the queue's deadline. Gating on
        // audio still playing would then strand the floor indefinitely.
        tokio::time::sleep_until(deadline.into()).await;
    } else {
        std::future::pending::<()>().await;
    }
}

async fn on_playout_settled(
    room: &RoomLink,
    context: &mut GeminiEventContext<'_>,
    loops: &mut RoomLoop,
    interview: InterviewContext<'_>,
) -> Result<ControlFlow<()>, Box<dyn std::error::Error + Send + Sync>> {
    if context.output_audio.is_playing() {
        return Ok(ControlFlow::Continue(()));
    }
    if !context.activity.generating {
        session::observe_floor(
            room.name(),
            context.activity,
            context.state,
            floor::Event::Drained,
        );
    }
    context.activity.mark_listening();
    set_agent_state(room, context.agent_state, AGENT_STATE_LISTENING);
    if spend_deferred_restart(room, context, loops, interview, "playout")
        .await?
        .is_break()
    {
        return Ok(ControlFlow::Break(()));
    }

    maybe_refresh_context(room, context).await;
    Ok(ControlFlow::Continue(()))
}

pub async fn run_room(
    config: &AgentConfig,
    room_name: &str,
    now_seconds: u64,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    match run_room_with_slot(config, room_name, now_seconds, None).await? {
        RoomRunOutcome::Completed | RoomRunOutcome::NoCandidate => Ok(()),
        RoomRunOutcome::FailedBeforeCandidate(error) => Err(error),
    }
}

#[derive(Debug)]
pub(crate) enum RoomRunOutcome {
    Completed,
    NoCandidate,
    FailedBeforeCandidate(Box<dyn std::error::Error + Send + Sync>),
}

fn startup_failure_line(
    model: &str,
    room_name: &str,
    session_id: u64,
    error: &(dyn std::error::Error + Send + Sync + 'static),
) -> String {
    // A socket may have opened before the failure, so no count is claimed; no
    // generation was asked for, so there is no usage.
    let outcome = LiveOutcome::from_error(error, LiveOutcome::Error);
    live_usage_line(
        room_name,
        session_id,
        model,
        0,
        outcome,
        None,
        &crate::gemini::TokenUsage::default(),
    )
}

pub(crate) async fn run_room_with_slot(
    config: &AgentConfig,
    room_name: &str,
    now_seconds: u64,
    slot: Option<&crate::dispatch::Slot>,
) -> Result<RoomRunOutcome, Box<dyn std::error::Error + Send + Sync>> {
    // Names this run on every usage line. Milliseconds rather than the
    // dispatch's seconds, so a room retried within the same second is not
    // summed with the run before it.
    let session_id = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| {
            u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
        });
    let keys = Arc::new(GeminiKeys::from_config(config));
    let session = open_session(
        config,
        &keys,
        room_name,
        now_seconds,
        session_id,
        |summary| {
            eprintln!("{summary}");
        },
    )
    .await?;
    let session = match session {
        OpenSessionOutcome::Started(session) => session,
        OpenSessionOutcome::NoCandidate => return Ok(RoomRunOutcome::NoCandidate),
        OpenSessionOutcome::FailedBeforeCandidate(error) => {
            return Ok(RoomRunOutcome::FailedBeforeCandidate(error));
        }
    };
    let OpenSession {
        room,
        events,
        agent_identity,
        candidate_identity,
        boot,
        mut output_audio,
        mut gemini,
        mut media,
        mut turn,
        started_at,
        restarts,
        deferred_events,
    } = *session;

    // The report handler takes exclusive ownership of this same queue after
    // End, including packets received while startup publication was pending.
    events.lock().await.deferred = deferred_events;
    let interview = InterviewContext {
        config,
        keys: &keys,
        boot: &boot,
        started_at,
        candidate_identity: &candidate_identity,
        events: &events,
        slot,
    };
    let ids = RoomIdentities {
        room_name,
        agent: &agent_identity,
        candidate: &candidate_identity,
        now_seconds,
    };
    let mut loops = RoomLoop {
        restarts,
        deferred_restart: DeferredRestart::default(),
        resume_recovery: ResumeRecovery::default(),
        interim_review: SideCall::default(),
        presence: CandidatePresence::default(),
    };

    // Held by the loop rather than by `media`, which is the candidate's inbound
    // tracks: a board is not a track, it arrives on the data channel, and the
    // one thing it shares with the camera is where it ends up.
    let mut board = Board::new();

    let mut watch = tokio::time::interval(Duration::from_secs_f64(WATCH_TICK_S));

    // The interview's own deadline, held by the process that owns the room
    // rather than by the candidate's tab. The browser countdown is a display:
    // it is throttled when the tab is hidden, and it stops entirely on an OS
    // suspend, so an interview whose only clock was the browser could run for
    // as long as the tab stayed open. This is also what bounds a metered Gemini
    // session when a candidate simply closes the tab.
    //
    // The grace is deliberate: the browser normally ends the interview itself,
    // and this only has to catch the case where it never does.
    let hard_deadline =
        tokio::time::sleep_until((started_at + interview_hard_deadline(boot.duration_min)).into());
    tokio::pin!(hard_deadline);

    let result: Result<(), Box<dyn std::error::Error + Send + Sync>> = async {
        loop {
            let step = tokio::select! {
                () = &mut hard_deadline, if !turn.state.ended => {
                    let mut context = turn.context(
                        &mut output_audio,
                        &mut gemini,
                        &mut board,
                        &mut media,
                    );
                    on_hard_deadline(&room, &mut context, &mut loops, interview).await?
                }
                _ = watch.tick(), if !turn.state.ended => {
                    let mut context = turn.context(
                        &mut output_audio,
                        &mut gemini,
                        &mut board,
                        &mut media,
                    );
                    on_watch_tick(&room, &mut context, &mut loops, interview, Instant::now()).await?
                }
                event = next_timed_room_event(&events) => {
                    let Some((received_at, event)) = event else {
                        eprintln!(
                            "LiveKit event stream ended for room={room_name}; ending with no report, \
                             because the room closed before the interview did"
                        );
                        gemini.shutdown().await?;
                        return Ok(());
                    };

                    // The board first, because taking the reader off the event
                    // is all this does with it: the stream is drained on its
                    // own task, and the loop hears about the board when there
                    // is a whole one.
                    if handle_board_event(
                        turn.state.interview_mode,
                        turn.state.example_drawing_enabled,
                        &candidate_identity,
                        &board,
                        &event,
                    ) {
                        ControlFlow::Continue(())
                    } else {
                        // Media next, because most events are, and because
                        // attaching a track needs the stream and the socket
                        // apart -- which is the one thing a context, which
                        // borrows both together, cannot give.
                        match handle_media_event(
                            &mut media,
                            &mut gemini,
                            &candidate_identity,
                            boot.candidate_video,
                            &event,
                        )
                        .await
                        {
                            Ok(true) => ControlFlow::Continue(()),
                            Ok(false) => {
                                let mut context = turn.context(
                                    &mut output_audio,
                                    &mut gemini,
                                    &mut board,
                                    &mut media,
                                );
                                handle_room_event(
                                    &room,
                                    &mut context,
                                    &mut loops.presence,
                                    interview,
                                    &ids,
                                    (received_at, event),
                                )
                                .await?
                            }
                            Err(error) => {
                                eprintln!("Gemini media attach failed ({error}); waiting for the close to be reported");
                                ControlFlow::Continue(())
                            }
                        }
                    }
                }
                event = gemini.next_event() => {
                    let mut context = turn.context(
                        &mut output_audio,
                        &mut gemini,
                        &mut board,
                        &mut media,
                    );
                    on_gemini_event(&room, &mut context, event, &mut loops, interview).await?
                }
                _ = wait_for_playout(turn.activity.floor, output_audio.playout_deadline) => {
                    let mut context = turn.context(
                        &mut output_audio,
                        &mut gemini,
                        &mut board,
                        &mut media,
                    );
                    on_playout_settled(&room, &mut context, &mut loops, interview).await?
                }
                frame = next_audio_frame(&mut media.audio), if media.audio.is_some() => {
                    // The fastest writer in the loop, and so the one that
                    // reaches a closed socket first: a flush leaves every ten
                    // milliseconds of speech. Dropping the frame costs a frame
                    // of audio the resumed session did not need; propagating
                    // cost the interview.
                    let ended = frame.is_none();

                    // Read by checkpoints and the idle timers before the first
                    // transcription, and during provider speech to distinguish
                    // ongoing input from its transcription tail.
                    let now = Instant::now();
                    if note_candidate_audio_frame(&turn.state, &mut turn.activity, frame.as_ref(), now) {
                        media.voiced_at = Some(now);
                    }
                    if let Err(error) = pump_audio(&mut media, &mut gemini, frame).await {
                        eprintln!("Gemini audio write failed ({error}); waiting for the close to be reported");
                    }

                    // One release for the arm, past every branch above it.
                    // Sitting inside a branch is what let a failed final flush
                    // keep an ended stream, and there is no path through here
                    // that wants to hold on to one.
                    release_if_ended(&mut media.audio, ended);
                    ControlFlow::Continue(())
                }
                Some(snapshot) = board.rx.recv(), if !turn.state.ended => {
                    if board::record(&mut board, &mut turn.state, snapshot) {
                        turn.activity.reset_completed_coding_idle();
                    }

                    // The candidate is working, even while silent. Without this
                    // the silence nudge counts a candidate who is drawing a
                    // diagram as idle and interrupts them mid-stroke, which is
                    // what `last_code_change` stops a typing candidate being
                    // asked.
                    let now = Instant::now();
                    turn.activity.last_code_change = now;
                    if let Err(error) = board::send_if_due(&mut board, &mut gemini, now).await {
                        eprintln!("Gemini board write failed ({error}); waiting for the close to be reported");
                    }
                    ControlFlow::Continue(())
                }
                frame = next_video_frame(&mut media.video), if media.video.is_some() => {
                    let speaking =
                        turn.activity.candidate_voice_active || media.voiced_recently(Instant::now());
                    if let Err(error) = pump_video(&mut media, &mut gemini, frame, speaking)
                    .await
                    {
                        eprintln!("Gemini video write failed ({error}); waiting for the close to be reported");
                    }
                    ControlFlow::Continue(())
                }
            };
            if media.audio.is_none() {
                turn.activity.clear_candidate_voice_activity();
            }
            if step.is_break() {
                return Ok(());
            }

            // Where the page hears about the hold. Every path that starts, ends
            // or re-announces one leaves its notice on the state, so none of
            // them can forget to tell the page. An ending flushes its own,
            // since the step that ends the interview leaves the room before
            // this runs.
            session::flush_thinking_notice(&room, &mut turn.state).await;
            session::reconcile_floor(
                room.name(),
                &mut turn.activity,
                || log_clock(&turn.state),
                output_audio.is_playing(),
                &mut std::io::stderr().lock(),
            );
            session::correct_agent_state(
                &room,
                &mut turn.agent_state,
                &turn.activity,
                output_audio.is_playing(),
            );
        }
    }
    .await;

    // Every exit from the loop lands here, including the ones that never close
    // the turns: the candidate not returning, the billing exits, the event
    // stream ending, an error. A cut still waiting loses only its quote there,
    // and this is the one place it is still printed, whatever exit is added.
    if let Some(line) = turn.turns.take_cut_line(Instant::now(), false) {
        eprintln!("{line}");
    }
    session::finish_turn_timing(
        room.name(),
        &mut turn.activity,
        &log_clock(&turn.state),
        timing::TimingOutcome::Ended,
        &mut std::io::stderr().lock(),
    );
    if let Err(error) = gemini.shutdown().await {
        eprintln!("Gemini close failed ({error}); recording received usage anyway");
    }
    session::drain_live_usage(
        &room,
        &mut turn.context(&mut output_audio, &mut gemini, &mut board, &mut media),
    );
    eprintln!("{}", turn.state.evidence_ledger.metrics.cost_line());
    let outcome = match &result {
        Err(error) => LiveOutcome::from_error(error.as_ref(), LiveOutcome::Error),
        Ok(()) => turn.activity.live_exit,
    };
    eprintln!(
        "{}",
        live_usage_line(
            room_name,
            session_id,
            boot.live_model,
            started_at.elapsed().as_secs(),
            outcome,
            Some(turn.activity.live_socket),
            &turn.activity.live_usage,
        )
    );
    result.map(|()| RoomRunOutcome::Completed)
}

/// A session's usage summary, in the one spelling the log analyzer reads.
/// `sockets` is `None` for an interview that failed before its first turn,
/// which may or may not have opened one, and says `phase=startup` instead.
fn live_usage_line(
    room: &str,
    session: u64,
    model: &str,
    elapsed_s: u64,
    outcome: LiveOutcome,
    sockets: Option<u64>,
    usage: &crate::gemini::TokenUsage,
) -> String {
    let lifetime = sockets.map_or_else(|| "phase=startup".to_string(), |n| format!("sockets={n}"));
    format!(
        "codetrial live_usage room={room} session={session} model={model} elapsed_s={elapsed_s} outcome={} {lifetime} {}",
        outcome.as_str(),
        usage.log_fields()
    )
}

/// One LiveKit room event that was not the candidate's media.
///
/// `Break` means the interview is over: the browser asked to end it, and the
/// report has already been published by the time this returns.
async fn handle_room_event(
    room: &RoomLink,
    context: &mut GeminiEventContext<'_>,
    presence: &mut CandidatePresence,
    interview: InterviewContext<'_>,
    ids: &RoomIdentities<'_>,
    (received_at, event): TimedRoomEvent,
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
            if handle_candidate_packet(room, context, presence, interview, topic, &payload)
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
            // Best effort: a duplicate left in place costs some confusion in
            // the room, while ending this loop over it cost the report.
            if let Err(error) =
                evict_duplicate_agent(config, room_name, &participant.identity().0, now_seconds)
                    .await
            {
                eprintln!(
                    "removing the duplicate agent failed ({error}); continuing the interview"
                );
            }
        }
        RoomEvent::ParticipantDisconnected(participant)
            if participant.identity().0 == candidate_identity =>
        {
            eprintln!("candidate left room={room_name}; waiting for a return");
            candidate_departed(presence, received_at);
        }
        RoomEvent::ParticipantConnected(participant)
            if participant.identity().0 == candidate_identity =>
        {
            candidate_returned(
                room,
                context,
                presence,
                interview.candidate_identity,
                received_at,
            )
            .await;
        }
        _ => {}
    }
    Ok(ControlFlow::Continue(()))
}

async fn handle_candidate_packet(
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

fn candidate_departed(presence: &mut CandidatePresence, received_at: Instant) {
    presence.left(received_at);
}

async fn candidate_returned(
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

// Reliable data is not a mailbox for an absent candidate. A return replays the
// successful probe, not its generation. The browser also queries on SDK resume,
// which need not produce a server-side ParticipantConnected event, and after
// its data channel is usable if an early return notice was missed again.
async fn resend_startup_readiness(
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
    let sent = async {
        let mut packet = browser_packet(TOPIC_CONTROL, &notice)?;
        packet.destination_identities = vec![candidate.into()];
        room.publish_data(packet).await
    };
    if !matches!(
        tokio::time::timeout(RECONNECT_NOTICE_LIMIT, sent).await,
        Ok(Ok(()))
    ) {
        eprintln!("startup state resync failed; room={}", room.name());
    }
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

    // The board is the only stream this agent is sent, so the room's ceiling on
    // one is the board's. Left at the SDK default it is five gigabytes, which
    // is a header away from a client that is not the browser buffering this
    // process to death before a single chunk is judged.
    let mut options = RoomOptions::default();
    options.data_stream = options
        .data_stream
        .with_max_payload_byte_length(MAX_BOARD_BYTES);
    crate::install_crypto_provider();
    let (room, events) = Room::connect(&config.livekit_url, &token, options).await?;
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
async fn leave_room(room: &RoomLink) {
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
    room: &RoomLink,
    notice: &serde_json::Value,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    room.publish_data(browser_packet(TOPIC_CONTROL, notice)?)
        .await?;
    Ok(())
}

/// How long the ending waits for queued transcript segments to reach the page
/// before it goes on. Normally they are out well within a second; a room that
/// cannot take them in this long will not take the report either, and the
/// report has its own recovery.
const TRANSCRIPT_FLUSH_LIMIT: Duration = Duration::from_secs(2);

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
    deferred: &mut Vec<TimedRoomEvent>,
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
                deferred.push((Instant::now(), other));
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
    let mut boot = crate::runtime::bootstrap_with_rounds(
        config,
        room_name,
        Some(candidate.problem.id),
        candidate.duration_min,
        crate::runtime::RuntimeOptions {
            voice: candidate.voice,
            accent: candidate.accent,
            profile: candidate.profile,
            grounding: candidate.grounding,
            interview_loop: candidate.interview_loop,
            examples_hidden: candidate.examples_hidden,
            example_drawing_enabled: candidate.example_drawing_enabled,
            candidate_led_enabled: candidate.candidate_led_enabled,
            interview_mode: candidate.interview_mode,
            code_execution_disabled: candidate.code_execution_disabled
                && !candidate.interview_mode.is_whiteboard(),
        },
    );
    if let Some(spoken) = candidate
        .language
        .as_deref()
        .and_then(crate::agent::spoken_language)
    {
        boot.greeting = crate::agent::greeting_with_language(spoken);
        boot.language = candidate.language;
    }
    boot
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
    keys: &'a Arc<GeminiKeys>,
    boot: &'a RuntimeBootstrap<'a>,
    started_at: Instant,
    candidate_identity: &'a str,
    events: &'a tokio::sync::Mutex<RoomEventReader>,
    slot: Option<&'a crate::dispatch::Slot>,
}

impl InterviewContext<'_> {
    /// Minutes since the interview started, as the report states its length.
    fn elapsed_min(&self) -> f64 {
        self.started_at.elapsed().as_secs_f64() / 60.0
    }
}

/// The interview's starting state, from the plan the token was minted for.
///
/// Every field here is read for the length of the interview and none can be
/// recovered once it is missed: the clock the round boundary is measured
/// against, the loop that decides whether a behavioral round exists at all,
/// and the two budgets the report divides the session into.
fn initial_runtime_state(boot: &RuntimeBootstrap<'_>, started_at: Instant) -> RuntimeState {
    let mut state = RuntimeState {
        started_at,
        interview_loop: boot.interview_loop,
        interview_mode: boot.interview_mode,
        code_execution_disabled: boot.code_execution_disabled,
        example_drawing_enabled: boot.example_drawing_enabled,
        candidate_led_enabled: boot.candidate_led_enabled,
        coding_minutes: boot.coding_minutes,
        behavioral_minutes: boot.behavioral_minutes,
        context_compression: boot.context_compression,
        ..RuntimeState::for_problem(boot.problem)
    };

    // Chosen before joining, so the browser's first packet in this language is
    // not a switch and no briefing asks for a language again.
    if let Some(language) = &boot.language {
        state.language = language.clone();
        state.language_chosen = true;
    }
    if boot.interview_loop == crate::agent::InterviewLoop::CodingBehavioral {
        state.evidence_ledger.set_uncovered_coverage(
            crate::agent::REACTO_PHASE_IDS
                .into_iter()
                .chain(crate::agent::STAR_PHASE_IDS),
        );
    }
    state
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

/// Whether the interviewer's request to close may be acted on yet.
///
/// A predicate rather than four conditions in the select arm, because three of
/// them are load-bearing in ways nothing else states. `interviewer_silenced`:
/// a thinking hold drops the interviewer's output, so a closing generated
/// during it is never heard and the candidate is handed a report out of a
/// silence they did not know had ended.
/// `tool_response_outstanding`: Gemini owes a generation for the tool response,
/// and acting before it lands means `send_wrap_up_and_wait` returns on the
/// acknowledgement's `TurnComplete` with the real closing cut off behind it.
/// `ended`: the report has already gone.
fn ready_to_close(state: &RuntimeState, activity: &RuntimeActivity) -> bool {
    state.end_requested
        && !state.ended
        && !state.interviewer_silenced()
        && !activity.socket.tool_response_outstanding
}

#[cfg(test)]
#[path = "../tests/unit/livekit.rs"]
mod tests;

#[cfg(test)]
#[path = "../tests/unit/livekit/cost.rs"]
mod cost_tests;
