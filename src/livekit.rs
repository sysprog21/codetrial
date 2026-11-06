//! The agent's side of a room: join, stream the candidate's audio and video to
//! Gemini, publish Gemini's audio back, and produce the report when the
//! interview ends.
//!
//! Its shape:
//!
//! `media` owns buffers and codecs, `turn` owns turn state, `rooms` talks to
//! the management API, and `report` builds the ending packet. `session` handles
//! Gemini events and outbound inputs; `packets` reduces browser controls;
//! `transcripts` queues publication to the page. `open` starts the interview,
//! `candidate` finds who joined, `restart` and `replacement` decide when and
//! how a Gemini socket is swapped, `side_calls` runs the reviews off the loop,
//! and `recovery` drives owned transport work. This file owns the room
//! lifecycle and the main select loop.

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

/// Whole seconds, rounded up, because LiveKit takes integers and a timeout
/// rounded down could undercut the grace it is derived from.
fn ceil_secs(duration: Duration) -> u32 {
    let whole = duration.as_secs() + u64::from(duration.subsec_nanos() > 0);
    u32::try_from(whole).unwrap_or(u32::MAX)
}

/// What the room's departure timeout adds to the absence limit and one watch
/// tick. LiveKit starts its clock when it removes the candidate and the agent
/// starts its own when it hears of it, then checks only on a tick, so without
/// slack the room could close just before the agent ends the interview.
const ROOM_DEPARTURE_MARGIN: Duration = Duration::from_secs(8);

/// How long LiveKit keeps an interview room open after the last candidate
/// leaves: the absence limit plus one watch tick plus a margin, so the agent's
/// own check fires and ends the interview before the room is closed under it.
/// LiveKit's default of 20 seconds closed the room long before the limit.
fn interview_room_departure_timeout_secs() -> u32 {
    ceil_secs(
        CANDIDATE_ABSENCE_LIMIT + Duration::from_secs_f64(WATCH_TICK_S) + ROOM_DEPARTURE_MARGIN,
    )
}

/// What the room's empty timeout adds to the agent's wait for its candidate.
/// The room is created in `/api/token`, before dispatch, agent start and the
/// agent's connection, and LiveKit counts an agent-only room as empty, so a
/// timeout equal to [`CANDIDATE_JOIN_LIMIT`] could close the room just before
/// the agent's own no-show deadline and turn that outcome into `RoomClosed`.
const ROOM_EMPTY_MARGIN: Duration = Duration::from_secs(30);

/// How long LiveKit keeps a room nobody joined: the agent's own wait for its
/// candidate plus a margin, so the agent's no-show path decides first.
fn interview_room_empty_timeout_secs() -> u32 {
    ceil_secs(CANDIDATE_JOIN_LIMIT + ROOM_EMPTY_MARGIN)
}

use crate::gemini::{
    GeminiEvent, GeminiKeys, GeminiLiveSession, generate_interim_review_with_keys,
    generate_phase_judgment_with_keys, live_session_with_keys,
};
use crate::runtime::{AGENT_NAME, RuntimeBootstrap, agent_identity};
use crate::token::{LivekitTokenInput, livekit_token};

mod board;
mod candidate;
mod floor;
mod latency;
mod media;
mod open;
mod packets;
#[cfg(test)]
mod probe;
mod recovery;
mod replacement;
mod report;
mod restart;
mod room_link;
mod room_service;
mod rooms;
mod session;
mod side_calls;
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
    send_model_context, send_model_context_scoped, send_model_text, send_wrap_up_and_wait,
    set_agent_state,
};

use board::{Board, MAX_BOARD_BYTES, handle_board_event};
pub use candidate::candidate_identity_matches;
use candidate::{
    candidate_bootstrap, candidate_metadata, is_duplicate_agent, is_interview_participant,
    join_room,
};
use open::{OpenSession, OpenSessionOutcome, open_cold_session_into, open_session};
use replacement::{
    Fallback, Replacement, Trigger, earlier_references, end_without_interviewer, fallback_reason,
    hand_over, handover_kind, hold_owed_reply, interviewer_state, prepare_recovery_brief,
    reconnect_wait, replace_gemini_session, replacement_fields, replacement_notice_line,
};
use report::{freeze_assessment, generate_report_bounded};
use restart::{
    DeferredRestart, LiveOutcome, ResumeRecovery, output_settled, replaced_socket_age,
    take_restart_attempt,
};
use rooms::{evict_duplicate_agent, isolate_local_agent};
use side_calls::{
    PHASE_JUDGE_SETTLE, SideCall, control_type, defers_round_transition, finish_phase_judgment,
    settle_phase_judge, settles_phase_judge, spawn_interim_review, spawn_phase_judge,
};

// `dispatch` tests drive the wait for a candidate through this path.
#[cfg(test)]
pub(crate) use candidate::arriving_candidate;
// Read by the test module through `use super::*`; nothing else needs them here.
#[cfg(test)]
use {
    open::{retry_cold_open, retry_cold_open_into},
    replacement::{
        Handover, brief_replacement, clear_abandoned_socket_work, keep_recovery_debt,
        replacement_notice, send_recovery_brief,
    },
    side_calls::take_interim_review_window,
};

// Re-exported rather than merely used: `web::setup` calls this to check the
// credentials a Setup form was given, and it names it through this module.
pub(crate) use rooms::validate_livekit_credentials;

pub(crate) use room_service::{create_interview_room, create_peer_waiting_room, delete_room};

// The test module reaches every item of these two through `use super::*`, so
// the hub keeps the globs instead of a list that grows with each test.
#[cfg_attr(
    not(test),
    expect(
        clippy::wildcard_imports,
        reason = "tests read the hub through super::*"
    )
)]
use media::*;
#[cfg_attr(
    not(test),
    expect(
        clippy::wildcard_imports,
        reason = "tests read the hub through super::*"
    )
)]
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
#[derive(Debug, Default)]
struct CandidatePresence {
    left_at: Option<Instant>,

    // Recovery tests advance this std-clock observation after entering a real
    // pending operation. It does not simulate SDK participant or tick delivery.
    #[cfg(test)]
    test_now: Option<Arc<std::sync::Mutex<Instant>>>,
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
        #[cfg(test)]
        let now = self
            .test_now
            .as_ref()
            .map_or(now, |clock| *clock.lock().unwrap());
        self.deadline(CANDIDATE_ABSENCE_LIMIT)
            .is_some_and(|deadline| now >= deadline)
    }

    /// When a grace of `limit` runs out, if the candidate is away. The report
    /// recovery wait holds the same rule with a shorter limit.
    fn deadline(&self, limit: Duration) -> Option<Instant> {
        self.left_at.map(|left| left + limit)
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

/// The room loop's activity, with the limits the operator configured fixed for
/// the whole interview.
fn runtime_activity(config: &AgentConfig, started_at: Instant, session_id: u64) -> RuntimeActivity {
    RuntimeActivity::for_interview(started_at, config.max_interim_reviews, session_id)
        .with_reply_timeout(Duration::from_secs(u64::from(
            config.gemini_reply_timeout_s,
        )))
        .with_first_output_deadline(Duration::from_secs(u64::from(config.gemini_first_output_s)))
}

/// Prepared-write gates a test hosting the real room loop arms before it
/// starts. `run_room_with_slot` builds its own `RoomLoop`, so this is the only
/// way a fixture reaches the gates the recovery matrices set on a hand-built
/// one. Empty unless a test fills it, and taken once by the loop that starts.
#[cfg(test)]
static LIVE_WRITE_BARRIERS: std::sync::Mutex<std::collections::VecDeque<recovery::WriteBarrier>> =
    std::sync::Mutex::new(std::collections::VecDeque::new());

#[cfg(test)]
fn take_live_write_barriers() -> std::collections::VecDeque<recovery::WriteBarrier> {
    std::mem::take(&mut *LIVE_WRITE_BARRIERS.lock().unwrap())
}

/// Where a test hosting the real room loop is told about each Live reader the
/// loop owns: the first socket's once the interview has started, then each
/// replacement's. Keyed by the room it hosts, so parallel tests never take one
/// another's, and taken once by the loop that starts. It is how a fixture
/// reaches the first reader to end it, which is the loop's own Close path, and
/// how it later sees that every reader has finished.
#[cfg(test)]
type LiveReaderProbe = (
    String,
    tokio::sync::mpsc::UnboundedSender<crate::gemini::LiveReaderInspection>,
);

#[cfg(test)]
static LIVE_READER_PROBES: std::sync::Mutex<Vec<LiveReaderProbe>> =
    std::sync::Mutex::new(Vec::new());

#[cfg(test)]
fn take_live_reader_probe(
    room: &str,
) -> Option<tokio::sync::mpsc::UnboundedSender<crate::gemini::LiveReaderInspection>> {
    let mut probes = LIVE_READER_PROBES.lock().unwrap();
    let at = probes.iter().position(|(name, _)| name == room)?;
    Some(probes.remove(at).1)
}

/// Where a test hosting the real room loop is told which terminal cancelled a
/// recovery wait, keyed by the room it hosts so parallel tests never see one
/// another's. Unlike the hand-built loop's `recovery_terminal`, it observes the
/// terminal and lets the loop handle it as production does.
#[cfg(test)]
type LiveTerminalProbe = (
    String,
    tokio::sync::mpsc::UnboundedSender<(&'static str, std::time::Instant)>,
);

#[cfg(test)]
static LIVE_TERMINAL_PROBES: std::sync::Mutex<Vec<LiveTerminalProbe>> =
    std::sync::Mutex::new(Vec::new());

#[cfg(test)]
fn note_live_terminal(room: &str, terminal: &'static str) {
    for (name, probe) in LIVE_TERMINAL_PROBES.lock().unwrap().iter() {
        if name == room {
            let _ = probe.send((terminal, std::time::Instant::now()));
        }
    }
}

/// The room loop's own bookkeeping, distinct from the session it runs on.
///
/// Four things only the loop reads, bundled so an arm that needs three of them
/// takes one parameter rather than three.
struct RoomLoop {
    pending_round_notice: Option<&'static str>,
    deadline_at: tokio::time::Instant,
    #[cfg(test)]
    recovery_terminal: Option<tokio::sync::mpsc::UnboundedSender<recovery::Terminal>>,
    #[cfg(test)]
    recovery_inputs: Option<tokio::sync::mpsc::UnboundedReceiver<recovery::Packet>>,
    #[cfg(test)]
    recovery_phase: Option<tokio::sync::mpsc::UnboundedSender<&'static str>>,
    #[cfg(test)]
    recovery_reader:
        Option<tokio::sync::mpsc::UnboundedSender<crate::gemini::LiveReaderInspection>>,
    #[cfg(test)]
    recovery_writes: std::collections::VecDeque<recovery::WriteBarrier>,
    #[cfg(test)]
    recovery_after: std::collections::VecDeque<recovery::AfterWrite>,
    /// Consecutive charged restart attempts, including cancelled opens.
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
) -> Result<ControlFlow<()>, Box<dyn std::error::Error + Send + Sync>> {
    // One reading of the clock for the whole tick. Three calls gave three
    // instants microseconds apart, so a cooldown stamped from one and tested
    // against another was answering a question nobody asked.
    let tick_at = Instant::now();
    if let Some(line) = context.activity.latency.clock_sample(tick_at) {
        eprintln!("{line}");
    }
    let alive_tasks = tokio::runtime::Handle::try_current()
        .ok()
        .map(|handle| handle.metrics().num_alive_tasks());
    if let Some(line) = context.activity.latency.task_sample(tick_at, alive_tasks) {
        eprintln!("{line}");
    }
    if loops.presence.gave_up(tick_at) {
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

    if let Some(status) = loops.pending_round_notice {
        let message =
            serde_json::json!({"type":"round_state", "round":"behavioral", "status":status});
        if let Ok(packet) = browser_packet(TOPIC_CONTROL, &message)
            && tokio::time::timeout(RECONNECT_NOTICE_LIMIT, room.publish_data(packet))
                .await
                .is_ok_and(|result| result.is_ok())
        {
            loops.pending_round_notice = None;
        }
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

async fn on_candidate_input(
    context: &mut GeminiEventContext<'_>,
    input: CandidateInput,
    silence_ms: u32,
) {
    let (frame, observed) = match input {
        CandidateInput::Frame(frame) => (Some(frame), true),
        CandidateInput::BufferedFrame(frame) => (Some(frame), false),
        CandidateInput::Yield => {
            if context.state.interviewer_silenced() {
                context.activity.ignore_input_before_hold(
                    Instant::now(),
                    thinking_transcript_grace(silence_ms),
                );
            }
            if let Err(error) = flush_candidate_audio(context.gemini, context.media).await {
                eprintln!(
                    "Gemini recovered audio flush failed ({error}); waiting for the close to be reported"
                );
                return;
            }
            if let Err(error) = context.gemini.end_audio_turn().await {
                eprintln!(
                    "Gemini recovered audio end failed ({error}); waiting for the close to be reported"
                );
            }
            return;
        }
        CandidateInput::End => (None, true),
    };
    let ended = frame.is_none();
    let now = Instant::now();
    if observed {
        context
            .media
            .note_mic_dequeue(&mut context.activity.latency, frame.is_some(), now);
    }
    if observed && note_candidate_audio_frame(context.state, context.activity, frame.as_ref(), now)
    {
        context.media.voiced_at = Some(now);
    }
    if let Err(error) = pump_audio(context.media, context.gemini, frame).await {
        eprintln!("Gemini audio write failed ({error}); waiting for the close to be reported");
    }
    release_if_ended(&mut context.media.audio, ended);
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
        if let Some(notice) = context
            .activity
            .latency
            .note_advisory(loops.deferred_restart.is_armed())
        {
            eprintln!(
                "Gemini requested a transport restart in {time_left} (notice {notice}); room={}",
                interview.boot.room_name
            );
        }

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
        return recovery::replace(room, context, interview, loops, Trigger::GoAway, None, true)
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
    let session = open_session(config, &keys, room_name, now_seconds, session_id, slot)
        .await
        .inspect_err(|error| {
            eprintln!(
                "{}",
                startup_failure_line(
                    &config.gemini_live_model,
                    room_name,
                    session_id,
                    error.as_ref(),
                )
            );
        })?;
    let session = match session {
        OpenSessionOutcome::Started(session) => session,
        OpenSessionOutcome::NoCandidate => return Ok(RoomRunOutcome::NoCandidate),
        OpenSessionOutcome::FailedBeforeCandidate(error) => {
            eprintln!(
                "{}",
                startup_failure_line(
                    &config.gemini_live_model,
                    room_name,
                    session_id,
                    error.as_ref(),
                )
            );
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
    } = *session;

    #[cfg(test)]
    let reader_probe = take_live_reader_probe(room_name);
    #[cfg(test)]
    if let Some(probe) = &reader_probe
        && let Some(reader) = gemini.inspect_reader_for_test()
    {
        let _ = probe.send(reader);
    }
    let interview = InterviewContext {
        config,
        keys: &keys,
        boot: &boot,
        #[cfg(test)]
        live_endpoint: None,
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
    let deadline_at = tokio::time::Instant::now() + interview_hard_deadline(boot.duration_min);
    let mut loops = RoomLoop {
        pending_round_notice: None,
        deadline_at,
        #[cfg(test)]
        recovery_terminal: None,
        #[cfg(test)]
        recovery_inputs: None,
        #[cfg(test)]
        recovery_phase: None,
        #[cfg(test)]
        recovery_reader: reader_probe,
        #[cfg(test)]
        recovery_writes: take_live_write_barriers(),
        #[cfg(test)]
        recovery_after: Default::default(),
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
    let hard_deadline = tokio::time::sleep_until(deadline_at);
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
                    on_watch_tick(&room, &mut context, &mut loops, interview).await?
                }
                event = async { events.lock().await.recv().await } => {
                    let Some(event) = event else {
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
                                    event,
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
                input = next_candidate_input(&mut media.audio, &mut media.recovery_audio), if media.audio.is_some() || !media.recovery_audio.is_empty() => {
                    let mut context = turn.context(&mut output_audio, &mut gemini, &mut board, &mut media);
                    on_candidate_input(&mut context, input, boot.silence_ms).await;
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
            #[cfg(test)]
            probe::note_retention(
                media.recovery_audio.dropped_bytes,
                media.quarantined_audio_bytes,
            );
            if media.quarantined_audio_bytes != 0 {
                eprintln!("candidate audio quarantine: bytes={} room={}",
                    std::mem::take(&mut media.quarantined_audio_bytes), room.name());
            }
            if media.audio.is_none() && media.recovery_audio.is_empty() {
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
    if let Some(line) = turn.activity.latency.mic_summary(Instant::now()) {
        eprintln!("{line}");
    }
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
            if handle_data_packet(room, context, interview, topic, &payload, true)
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
            presence.left(Instant::now());
        }
        RoomEvent::ParticipantConnected(participant)
            if participant.identity().0 == candidate_identity =>
        {
            presence.returned();
            context.state.announce_thinking();
            // A rejoined page starts with an empty checklist.
            context.state.framework_published.clear();
        }
        _ => {}
    }
    Ok(ControlFlow::Continue(()))
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
    #[cfg(test)]
    live_endpoint: Option<&'a str>,
    started_at: Instant,
    candidate_identity: &'a str,
    events: &'a tokio::sync::Mutex<tokio::sync::mpsc::UnboundedReceiver<RoomEvent>>,
    slot: Option<&'a crate::dispatch::Slot>,
}

impl InterviewContext<'_> {
    async fn open_live(
        self,
        handle: Option<(&str, &str)>,
    ) -> Result<GeminiLiveSession, Box<dyn std::error::Error + Send + Sync>> {
        #[cfg(test)]
        if let Some(endpoint) = self.live_endpoint {
            return crate::gemini::live_session_with_keys_at(
                endpoint, self.keys, self.boot, handle,
            )
            .await;
        }
        live_session_with_keys(self.keys, self.boot, handle).await
    }

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
