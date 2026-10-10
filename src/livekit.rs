//! The agent's side of a room: join, stream the candidate's audio and video to
//! Gemini, publish Gemini's audio back, and produce the report when the
//! interview ends.
//!
//! Still the largest module in the tree, so here is its shape.
//!
//! Five regions are out. `media` holds the buffers, pumps and codecs that carry
//! audio and video in both directions, and `turn` holds the state a turn is
//! made of, which explains what stayed behind with the loop. `rooms` is the
//! only region that talks to LiveKit over HTTP rather than over the room's
//! event stream, `report` is the packet an ending interview produces, and
//! `session` is everything that hangs off one inbound Gemini event or one
//! outbound text: the handlers, the tool dispatch, the transcript publishing
//! and the turn procedures that end a turn.
//!
//! What is left is the room and the loop that drives it:
//!
//! - `run_room` and `join_room`: the lifecycle, and the only entry point.
//! - Session setup and restart (`take_restart_attempt` through `open_session`):
//!   what runs once before the loop, and what the loop calls when the Gemini
//!   socket dies under it.
//! - `handle_data_packet`: what the browser sends. The other two sources are
//!   elsewhere -- `handle_gemini_event` in `session`, `handle_media_event` in
//!   `media`.
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
mod media;
mod report;
mod rooms;
mod session;
mod turn;

// The room half reaches back for these on every inbound event, which is what
// makes the split a file boundary rather than a layer: the select loop owns the
// room and hands one borrow of it over for the length of one Gemini event.
pub use session::execute_tool_call;
use session::{
    GeminiEventContext, TestRunLine, TurnCause, close_turns, cut_off_turn, handle_gemini_event,
    log_clock, prompt_fields, prompt_line, send_model_context, send_model_text,
    send_wrap_up_and_wait, set_agent_state,
};

use board::{Board, MAX_BOARD_BYTES, handle_board_event};
use report::{freeze_assessment, generate_report_bounded};
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
        if completed_live_reply(state, activity, event) {
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
    fn take_if_settled(&mut self, activity: &RuntimeActivity, audio_playing: bool) -> bool {
        self.take_if_due(
            activity.floor,
            audio_playing,
            activity.tool_response_outstanding,
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
    room: Option<&Room>,
) -> Result<GeminiLiveSession, Box<dyn std::error::Error + Send + Sync>> {
    retry_cold_open(
        interview.keys,
        restarts,
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
    room: &Room,
    context: &mut GeminiEventContext<'_>,
    interview: InterviewContext<'_>,
    loops: &mut RoomLoop,
    reply_timeout: bool,
) -> Result<ControlFlow<()>, Box<dyn std::error::Error + Send + Sync>> {
    // Neither a resume nor a cold open on the same key can get past a project
    // that cannot pay, so the budget is not spent learning that twice more.
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
    let age = replaced_socket_age(context.gemini.age(), reply_timeout, context.activity);
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
    let handle = handle.filter(|_| allow_resume);

    // The cold-rebuild line tells the two causes of a 1011 run apart: a resumed
    // conversation that keeps failing stops failing once it is rebuilt, while
    // input that fails on any socket fails the rebuild too.
    if loops.resume_recovery.suppressed {
        eprintln!(
            "{}; rebuilding from local state room={}",
            if loops.resume_recovery.resumed {
                "Gemini resumed session failed before recovery"
            } else {
                "Gemini cold rebuild also failed before recovery"
            },
            interview.boot.room_name
        );
    }

    // Said before the attempt, not after it: the whole point is to cover the
    // gap, and the gap starts here. Connect and setup are bounded at fifteen
    // seconds each, and for that long the candidate is talking to a socket that
    // is gone.
    publish_interviewer_state(room, &interviewer_state(true)).await?;

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
    loops.resume_recovery.connected(resumed);
    context.activity.live_socket += 1;
    context.activity.reset_context_observations(resumed);

    let (owed, debt, owed_prompt) =
        hand_over(context.state, context.activity, context.output_audio);

    // The one line that ties a replacement to what the new socket is missing:
    // how old the checkpoint it resumed from is, what was owed, and what the
    // local record holds that the checkpoint may predate.
    eprintln!(
        "replacement: at={} resumed={resumed} checkpoint_age={checkpoint_age} owed={owed} ({debt}) owed_chars={} editor_chars={} {} room={}",
        log_clock(context.state),
        owed_prompt
            .as_deref()
            .map_or(0, |prompt| prompt.chars().count()),
        context.state.code.chars().count(),
        prompt_fields(context.state, context.activity),
        interview.boot.room_name
    );
    close_turns(room, context).await?;
    set_agent_state(room, context.agent_state, AGENT_STATE_LISTENING).await?;
    publish_interviewer_state(room, &interviewer_state(false)).await?;
    let spoke = brief_replacement(
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
    .await;

    // A cold session remembers none of the drawing, and the briefing's text
    // cannot carry it. Sent whether or not the briefing itself was held for a
    // pause, so the session that eventually speaks has seen the board.
    if !resumed && let Err(error) = board::resend(context.board, context.gemini).await {
        eprintln!("cold-restart board failed ({error}); waiting for the close to be reported");
    }
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
    room: &Room,
    context: &mut GeminiEventContext<'_>,
    interview: InterviewContext<'_>,
    loops: &mut RoomLoop,
) -> Result<ControlFlow<()>, Box<dyn std::error::Error + Send + Sync>> {
    // A replacement that gave up after announcing itself would leave the
    // reconnecting notice up over the report. Not `?`: the report matters more
    // than the notice.
    if let Err(error) = publish_interviewer_state(room, &interviewer_state(false)).await {
        eprintln!("clearing the reconnecting notice failed ({error}); writing the report anyway");
    }
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
        activity.tool_response_outstanding,
        activity.generating,
    );
    let owed = activity.reply_unfinished();
    cut_off_turn(activity, output_audio);
    clear_abandoned_socket_work(state, activity);
    activity.evidence_shown = None;
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

/// Tells a replacement socket what it cannot know on its own, and reports
/// whether the briefing asked for a reply.
///
/// A resumed checkpoint is the last turn boundary the server marked resumable,
/// so it may predate the latest test run or spoken answer; a cold one knows
/// nothing. Both get the local record. Kept apart from `replace_gemini_session`
/// so a test drives the same branch production does, log line included.
async fn brief_replacement(
    gemini: &mut GeminiLiveSession,
    state: &mut RuntimeState,
    activity: &mut RuntimeActivity,
    replacement: Replacement,
    owed_prompt: Option<&str>,
) -> bool {
    if matches!(replacement, Replacement::Resumed { .. }) {
        // Matched literally by the soak in scripts/browser-check.cjs, which has
        // no other way to tell a resumption from a cold replacement: both leave
        // the interviewer in the room. Rewording this line without moving that
        // one turns the soak's only positive signal into a timeout.
        eprintln!("Gemini session resumed; the interview continues where it left off");
    } else {
        eprintln!(
            "Gemini session restart degraded; resumption was not used and the interviewer is rebuilding from local transcript, editor and interview state"
        );
    }

    // Not `?`, for the reason the watch loop already gives about writes to
    // Gemini: a socket that came up and died again reports the close through
    // `next_event`, where this arm is waiting to restart it. Propagating here
    // would end the interview on the one write the restart exists to make, and
    // the write most likely to meet a socket that is already gone.
    match send_recovery_brief(gemini, state, replacement, owed_prompt).await {
        Ok(true) => {
            // The briefing is scaffolding, not the event: a socket that drops
            // before answering it still owes what the briefing asked about, and
            // naming the briefing there would nest it, transcript and all,
            // inside the next one.
            activity.mark_prompted(Instant::now(), owed_prompt, false);
            state.recovery_reply_pending = true;
            true
        }
        Ok(false) => false,
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
/// socket always speaks, except during a pause, where the reply would be
/// discarded: the briefing is owed until unpause instead. A resumed socket
/// whose cold predecessor still owed its briefing gets that briefing, since
/// its memory starts from the cold session.
async fn send_recovery_brief(
    gemini: &mut GeminiLiveSession,
    state: &mut RuntimeState,
    replacement: Replacement,
    owed_prompt: Option<&str>,
) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
    // A reply owed during a pause or a hold cannot be asked for yet, since it
    // would be dropped; ending it asks for it instead, on either kind of
    // socket.
    let owed = replacement.owed();
    let held = state.floor_held();
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
        send_model_context(
            gemini,
            state,
            ModelInputKind::Turn,
            &context,
            reply.then_some(TurnCause::Recovery),
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
        // pause or hold ends.
        if state.paused {
            // Nothing reaches the new socket until the pause ends, so its
            // briefing can wait for the resume.
            state.needs_cold_brief = true;
            return Ok(false);
        }

        // A hold is not a pause: the candidate's audio still reaches the new
        // socket, which could answer them knowing nothing of the interview.
        // Briefed now, as context that asks for no answer.
        let briefing = crate::agent::with_timer(state, crate::agent::cold_restart(state));
        state.code_shown = state.code.clone();
        send_model_context(gemini, state, ModelInputKind::Turn, &briefing, None).await?;
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
    send_model_text(
        gemini,
        state,
        ModelInputKind::Turn,
        TurnCause::Recovery,
        &briefing,
    )
    .await?;
    state.clear_thinking_debt();
    Ok(true)
}

/// Keeps a reply owed during a pause for the unpause to ask for, whether the
/// pause found it owed or a replacement during the pause did. A known event is
/// never replaced by an unknown one: a candidate transcript landing in the
/// pause leaves `hand_over` no prompt text, but the event it was paused on is
/// still the one the reply belongs to.
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
    activity.discarding_output = false;
    state.recovery_reply_pending = false;

    // A provisional request waits on its utterance's end, which the closed
    // socket will not send. A declared hold is the candidate's and survives.
    state.withdraw_thinking_request();
    activity.reply_after_thinking_discard = false;
    activity.thinking_reply_fallback = None;
    activity.thinking_ignore_input_until = None;
    activity.interrupted_turn_pending = false;
    activity.stale_turn_pending = false;
    activity.tool_response_outstanding = false;
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
/// candidate's audio, whose queue holds about a tenth of a second, so anything
/// said meanwhile never reaches the interviewer; at the end nothing more is
/// owed a reply, but the round transition is held instead, see
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
        &crate::agent::mark_unrecognized_turns(&state.transcript[unreviewed_from(state)..]),
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
    room: Room,
    events: tokio::sync::Mutex<tokio::sync::mpsc::UnboundedReceiver<RoomEvent>>,
    agent_identity: String,
    candidate_identity: String,
    boot: RuntimeBootstrap<'a>,
    output_audio: OutputAudio,
    gemini: crate::gemini::GeminiLiveSession,
    media: CandidateMedia,
    turn: TurnState,
    started_at: Instant,
    restarts: usize,
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
    keys: &Arc<GeminiKeys>,
    room_name: &'a str,
    now_seconds: u64,
    session_id: u64,
) -> Result<Option<OpenSession<'a>>, Box<dyn std::error::Error + Send + Sync>> {
    let agent_identity = agent_identity(room_name);
    let (room, mut events) = join_room(config, room_name, &agent_identity, now_seconds).await?;
    let mut agent_state = String::new();
    session::publish_opening_attributes(&room, &mut agent_state, config.gemini_silence_ms).await?;

    // The candidate's metadata picks the problem, so nothing else can start
    // until someone joins.
    let mut deferred_events = Vec::new();
    let Some((candidate_identity, candidate_metadata)) =
        candidate_metadata(&room, &mut events, &mut deferred_events).await?
    else {
        eprintln!("no candidate joined room={room_name}; leaving it");
        return Ok(None);
    };
    let events = tokio::sync::Mutex::new(events);
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
    let mut restarts = 0;
    let (output_audio, mut gemini) = tokio::try_join!(
        async {
            isolate_local_agent(config, room_name, &agent_identity, now_seconds).await?;
            publish_output_audio(&room, GEMINI_OUTPUT_AUDIO_SAMPLE_RATE).await
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
            crate::gemini::first_open_within(
                crate::gemini::FIRST_OPEN_LIMIT,
                open_cold_session(interview, &mut restarts, None),
            )
            .await
        },
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
        activity: runtime_activity(config, started_at, session_id),
        turns: SpeakerTurns::default(),
    };
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
    send_model_text(
        &mut gemini,
        &mut turn.state,
        ModelInputKind::Turn,
        TurnCause::Turn,
        &greeting,
    )
    .await?;
    turn.activity
        .mark_prompted(Instant::now(), Some(&greeting), false);

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
        restarts,
    }))
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
    room: &Room,
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
async fn maybe_refresh_context(room: &Room, context: &mut GeminiEventContext<'_>) {
    // Runs after every Gemini event, most of them audio. Nothing pending is the
    // common case, and the gates below read the candidate's open turn to say
    // no, so it is answered first.
    if !context.activity.context_refresh_pending {
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
        match send_model_context(
            context.gemini,
            context.state,
            ModelInputKind::Turn,
            &checkpoint,
            None,
        )
        .await
        {
            Ok(()) => {
                context.activity.context_refresh_pending = false;
                eprintln!(
                    "codetrial context_refresh room={} session={} bytes={}",
                    room.name(),
                    context.activity.live_session_id,
                    checkpoint.len()
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
    room: &Room,
    context: &mut GeminiEventContext<'_>,
    loops: &mut RoomLoop,
    interview: InterviewContext<'_>,
) -> Result<ControlFlow<()>, Box<dyn std::error::Error + Send + Sync>> {
    // One reading of the clock for the whole tick. Three calls gave three
    // instants microseconds apart, so a cooldown stamped from one and tested
    // against another was answering a question nobody asked.
    let tick_at = Instant::now();
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

    // A board the send interval held back, or one that arrived during a pause,
    // goes out here once nothing stops it. Without this it waited for the next
    // board to carry it, and the candidate who has stopped drawing to explain
    // is exactly the one who sends no next board.
    if !context.state.paused
        && let Err(error) = board::send_if_due(context.board, context.gemini, tick_at).await
    {
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
    if reply_watch(
        context.state,
        context.activity,
        tick_at,
        context.output_audio.is_playing(),
    ) == ReplyWatch::Recover
    {
        loops.deferred_restart.cancel();
        eprintln!(
            "Gemini reply timed out after {}s; reconnecting at={} room={}",
            context.activity.reply_timeout.as_secs(),
            log_clock(context.state),
            interview.boot.room_name
        );
        return replace_gemini_session(room, context, interview, loops, true).await;
    }
    if stalls.spend_restart
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
    // publishes `speaking`, a completed or cut turn, a replacement and a pause
    // publish `listening`, and so does the candidate speaking again.
    if reply == (ReplyWatch::Owed { shown: true }) {
        set_agent_state(room, context.agent_state, AGENT_STATE_THINKING).await?;
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

fn completed_live_reply(
    state: &RuntimeState,
    activity: &RuntimeActivity,
    event: &GeminiEvent,
) -> bool {
    matches!(event, GeminiEvent::TurnComplete)
        && activity.generating
        && !activity.discarding_output
        && !state.paused
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

/// While the floor is held, by a pause or by the candidate keeping it to
/// think, nothing is waited on and nothing is shown: the interviewer's silence
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
    let watched = !state.floor_held() && !state.end_requested;
    if watched && activity.reply_timed_out(now, audio_playing) {
        return ReplyWatch::Recover;
    }
    ReplyWatch::Owed {
        shown: watched && activity.reply_visibly_late(now, audio_playing),
    }
}

/// Pausing cancels the old turn even if no output has taken the floor yet.
/// Resuming rebases debt that provider events created during the pause.
fn update_pause_activity(
    state: &mut RuntimeState,
    activity: &mut RuntimeActivity,
    output_audio: &mut OutputAudio,
    now: Instant,
) {
    if !state.paused {
        activity.resume_reply_wait(now);
        return;
    }
    if activity.reply_unfinished() {
        let owed_prompt = activity
            .prompt_text
            .as_deref()
            .filter(|_| activity.owes_prompt());
        hold_owed_reply(state, owed_prompt);
    }
    activity.discarding_output = pause_leaves_output_in_flight(activity, now);
    activity.stale_turn_pending = activity.generating && !activity.discarding_output;
    cut_off_turn(activity, output_audio);
}

/// The event a resume owes a reply for, read before the resume prompt is
/// composed around it. The cold briefing the resume may also carry stays in
/// the state until a send pays it, so it needs no copy here.
struct ResumeDebt {
    owed_prompt: Option<String>,
}

impl ResumeDebt {
    fn capture(state: &RuntimeState) -> Option<Self> {
        state.paused.then(|| Self {
            owed_prompt: state.owed_reply_on_resume.clone().flatten(),
        })
    }
}

/// A failed resume must restore the raw debt, never the composed briefing.
fn record_event_prompt<E>(
    state: &mut RuntimeState,
    activity: &mut RuntimeActivity,
    prompt: &str,
    resume_debt: Option<&ResumeDebt>,
    sent: Result<(), E>,
    now: Instant,
) -> Result<(), E> {
    match sent {
        Ok(()) => {
            let owed_prompt = match resume_debt {
                Some(debt) => debt.owed_prompt.as_deref(),
                None => Some(prompt),
            };
            activity.mark_prompted(now, owed_prompt, false);
            Ok(())
        }
        Err(error) => {
            // The unpause leaves its debt in the state until a send pays it.
            // The owed event moves onto the activity, where the next socket's
            // briefing names it once; left in both, it was named twice.
            if let Some(debt) = resume_debt {
                state.owed_reply_on_resume = None;
                activity.owe_prompt(now, debt.owed_prompt.clone());
            }
            Err(error)
        }
    }
}

/// Failed writes leave the behavioral invitation available after reconnect.
/// Cooldowns still bound retries while the old socket reports its close.
async fn send_watched_prompt<E>(
    activity: &mut RuntimeActivity,
    prompt: &WatchPrompt,
    send: impl std::future::Future<Output = Result<(), E>>,
) -> Result<(), E> {
    send.await?;
    if prompt.behavioral_nudge {
        activity.behavioral_nudged = true;
    }
    activity.mark_prompted(Instant::now(), Some(&prompt.text), prompt.allows_silence);
    Ok(())
}

/// One Gemini event, plus the two restarts only this arm can decide.
async fn on_gemini_event(
    room: &Room,
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
        if replace_gemini_session(room, context, interview, loops, false)
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
            context.activity.reply_in_flight(),
            context.activity.tool_response_outstanding,
        ) {
            eprintln!(
                "goaway held: at={} floor={:?} playing={} reply_pending={} tool_owed={} room={}",
                log_clock(context.state),
                context.activity.floor,
                context.output_audio.is_playing(),
                context.activity.reply_in_flight(),
                context.activity.tool_response_outstanding,
                interview.boot.room_name
            );
            return Ok(ControlFlow::Continue(()));
        }
        if replace_gemini_session(room, context, interview, loops, false)
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
    // beat, not a condition that can hold the interview open. Not while paused.
    // The closing would be generated into a room whose output this loop drops
    // (`output_disposition`), so the candidate hears none of it and is handed a
    // report out of a silence they did not know had ended. Pausing also clears
    // the outstanding-response flag, so without this the next event of any kind
    // ends the interview. Held instead until they come back; if they never do,
    // the deadline still ends it.
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
    room: &Room,
    context: &mut GeminiEventContext<'_>,
    loops: &mut RoomLoop,
    interview: InterviewContext<'_>,
    trigger: &str,
) -> Result<ControlFlow<()>, Box<dyn std::error::Error + Send + Sync>> {
    if !loops
        .deferred_restart
        .take_if_settled(context.activity, context.output_audio.is_playing())
    {
        return Ok(ControlFlow::Continue(()));
    }
    eprintln!(
        "goaway spent: trigger={trigger} at={} room={}",
        log_clock(context.state),
        interview.boot.room_name
    );
    replace_gemini_session(room, context, interview, loops, false).await
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
    room: &Room,
    context: &mut GeminiEventContext<'_>,
    loops: &mut RoomLoop,
    interview: InterviewContext<'_>,
) -> Result<ControlFlow<()>, Box<dyn std::error::Error + Send + Sync>> {
    if context.output_audio.is_playing() {
        return Ok(ControlFlow::Continue(()));
    }
    context.activity.mark_listening();
    set_agent_state(room, context.agent_state, AGENT_STATE_LISTENING).await?;
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
    run_room_with_slot(config, room_name, now_seconds, None).await
}

pub(crate) async fn run_room_with_slot(
    config: &AgentConfig,
    room_name: &str,
    now_seconds: u64,
    slot: Option<&crate::dispatch::Slot>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Names this run on every usage line. Milliseconds rather than the
    // dispatch's seconds, so a room retried within the same second is not
    // summed with the run before it.
    let session_id = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| {
            u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
        });
    let keys = Arc::new(GeminiKeys::from_config(config));
    let Some(OpenSession {
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
    }) = open_session(config, &keys, room_name, now_seconds, session_id)
        .await
        .inspect_err(|error| {
            // The interview never started, and the summary is where an operator
            // counts those, a first open a depleted project refused above all.
            // A socket may have opened before the failure, so no count is
            // claimed; no generation was asked for, so there is no usage.
            let outcome = LiveOutcome::from_error(error.as_ref(), LiveOutcome::Error);
            eprintln!(
                "{}",
                live_usage_line(
                    room_name,
                    session_id,
                    &config.gemini_live_model,
                    0,
                    outcome,
                    None,
                    &crate::gemini::TokenUsage::default(),
                )
            );
        })?
    else {
        return Ok(());
    };

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
    let hard_deadline = tokio::time::sleep_until(
        (Instant::now() + interview_hard_deadline(boot.duration_min)).into(),
    );
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
                            board.allows_camera(boot.candidate_video),
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
                frame = next_audio_frame(&mut media.audio), if media.audio.is_some() => {
                    // The fastest writer in the loop, and so the one that
                    // reaches a closed socket first: a flush leaves every
                    // hundred milliseconds of speech. Dropping the frame costs
                    // a tenth of a second of audio the resumed session did not
                    // need; propagating cost the interview.
                    let ended = frame.is_none();

                    // Read by a checkpoint under a compression window, and by
                    // the idle timers until the socket has transcribed the
                    // candidate; otherwise the per-frame level is not worth
                    // computing.
                    if (turn.state.context_compression.is_some()
                        || !turn.activity.transcribed_on_socket)
                        && frame.as_ref().is_some_and(media::frame_has_voice)
                    {
                        turn.activity.note_candidate_voice(Instant::now());
                    }
                    if turn.state.paused {
                        discard_paused_audio(&mut media);
                    } else if let Err(error) = pump_audio(&mut media, &mut gemini, frame).await {
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
                    // Kept even while paused. The board is the drawing as it
                    // stands, and one exported just before the pause is work
                    // the candidate did; dropping it left the interviewer on
                    // the board before it until another edit, which a paused
                    // candidate cannot make. It waits to be shown until the
                    // interview resumes, through the watch tick.
                    board::record(&mut board, &mut turn.state, snapshot);

                    // Stop an existing camera stream too; refusing future
                    // attachments alone leaves this one overwriting drawings.
                    media.video = None;
                    if !turn.state.paused {
                        // The candidate is working, even while silent. Without
                        // this the silence nudge counts a candidate who is
                        // drawing a diagram as idle and interrupts them
                        // mid-stroke, which is what `last_code_change` stops a
                        // typing candidate being asked.
                        let now = Instant::now();
                        turn.activity.last_code_change = now;
                        if let Err(error) = board::send_if_due(&mut board, &mut gemini, now).await {
                            eprintln!("Gemini board write failed ({error}); waiting for the close to be reported");
                        }
                    }
                    ControlFlow::Continue(())
                }
                frame = next_video_frame(&mut media.video), if media.video.is_some() => {
                    if turn.state.paused {
                        release_if_ended(&mut media.video, frame.is_none());
                    } else if let Err(error) = pump_video(&mut media, &mut gemini, frame).await {
                        eprintln!("Gemini video write failed ({error}); waiting for the close to be reported");
                    }
                    ControlFlow::Continue(())
                }
            };
            if step.is_break() {
                return Ok(());
            }

            // Where the page hears about the hold. Every path that starts, ends
            // or re-announces one leaves its notice on the state, so none of
            // them can forget to tell the page. An ending flushes its own,
            // since the step that ends the interview leaves the room before
            // this runs.
            session::flush_thinking_notice(&room, &mut turn.state).await;
        }
    }
    .await;
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
    result
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
            context.state.announce_thinking();
            // A rejoined page starts with an empty checklist.
            context.state.framework_published.clear();
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

    // The board is the only stream this agent is sent, so the room's ceiling on
    // one is the board's. Left at the SDK default it is five gigabytes, which
    // is a header away from a client that is not the browser buffering this
    // process to death before a single chunk is judged.
    let mut options = RoomOptions::default();
    options.data_stream = options
        .data_stream
        .with_max_payload_byte_length(MAX_BOARD_BYTES);
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
    notice: &serde_json::Value,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    room.local_participant()
        .publish_data(browser_packet(TOPIC_CONTROL, notice)?)
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
            examples_hidden: candidate.examples_hidden,
            interview_mode: candidate.interview_mode,
            code_execution_disabled: candidate.code_execution_disabled
                && !candidate.interview_mode.is_whiteboard(),
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
    keys: &'a Arc<GeminiKeys>,
    boot: &'a RuntimeBootstrap<'a>,
    started_at: Instant,
    candidate_identity: &'a str,
    events: &'a tokio::sync::Mutex<tokio::sync::mpsc::UnboundedReceiver<RoomEvent>>,
    slot: Option<&'a crate::dispatch::Slot>,
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
        coding_minutes: boot.coding_minutes,
        behavioral_minutes: boot.behavioral_minutes,
        context_compression: boot.context_compression,
        ..RuntimeState::for_problem(boot.problem)
    };
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
/// them are load-bearing in ways nothing else states. `paused`: the closing
/// would be generated into a room whose output this loop drops, so the
/// candidate hears none of it and is handed a report out of a silence they did
/// not know had ended -- and pausing also clears the flag below, so without
/// this the next event of any kind would end the interview.
/// `tool_response_outstanding`: Gemini owes a generation for the tool response,
/// and acting before it lands means `send_wrap_up_and_wait` returns on the
/// acknowledgement's `TurnComplete` with the real closing cut off behind it.
/// `ended`: the report has already gone.
fn ready_to_close(state: &RuntimeState, activity: &RuntimeActivity) -> bool {
    state.end_requested
        && !state.ended
        && !state.floor_held()
        && !activity.tool_response_outstanding
}

/// The candidate chose Thinking at `at`. Ending the audio stream makes Gemini
/// transcribe what came before the click now, inside the window
/// `ignore_input_before_hold` opens, rather than a silence window later; in a
/// measured session the transcript followed the stream end by about 0.3s. The
/// reply Gemini starts for it is dropped by the hold.
async fn begin_button_hold(
    gemini: &mut GeminiLiveSession,
    candidate_audio: &mut Vec<u8>,
    activity: &mut RuntimeActivity,
    at: Instant,
    grace: Duration,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    activity.ignore_input_before_hold(at, grace);
    flush_audio(gemini, candidate_audio).await?;
    gemini.end_audio_turn().await
}

/// Told to the model when a Thinking click cuts off a reply it was giving.
const THINKING_CUT_OFF: &str = "[SYSTEM EVENT] The candidate is thinking and has kept the floor. Any reply interrupted by this hold was not heard in full. Stay silent until they speak again or yield the turn.";

/// What a control packet's thinking outcome leaves for the room to do.
#[derive(Debug, Default, PartialEq, Eq)]
struct HoldEffects {
    /// Jim was cut off: his turn has to be closed and his state shown as
    /// listening.
    cut_off: bool,
    /// Finalizing the candidate's turn failed and its reply is marked owed,
    /// so nothing more is sent for this packet.
    abandoned: bool,
}

/// The room-free half of a control packet's thinking outcome: the socket
/// writes and the activity bookkeeping, split from `handle_data_packet` so a
/// fake socket can drive it. `reply` is the prompt the packet asks for, and
/// comes back `None` when this has already delivered it as context.
///
/// A new hold ends the audio stream and cuts off a reply in progress. Ending
/// a hold with something to say, or yielding, finalizes the candidate's turn:
/// buffered audio goes, then the stream ends so Gemini answers without waiting
/// out its silence window. A candidate turn still open takes the prompt as
/// context instead, since a realtime prompt would answer before the
/// candidate's own words.
async fn settle_hold(
    context: &mut GeminiEventContext<'_>,
    result: &crate::agent::DataEventResult,
    reply: &mut Option<String>,
    at: Instant,
    grace: Duration,
) -> HoldEffects {
    let mut effects = HoldEffects::default();
    if result.thinking_changed == Some(true) {
        if let Err(error) = begin_button_hold(
            context.gemini,
            &mut context.media.audio_bytes,
            context.activity,
            at,
            grace,
        )
        .await
        {
            eprintln!(
                "Gemini audio stream end failed ({error}); waiting for the close to be reported"
            );
        }
        if context.activity.floor != Floor::Listening {
            session::cut_off_for_hold(context.activity, context.output_audio);

            // Owed until a prompt that carries it is sent: the note below can
            // fail to send, and a resumed socket keeps the partial reply.
            context.state.thinking_unheard_reply = true;
            effects.cut_off = true;
            if let Err(error) = send_model_context(
                context.gemini,
                context.state,
                ModelInputKind::Turn,
                THINKING_CUT_OFF,
                None,
            )
            .await
            {
                eprintln!(
                    "Gemini thinking context failed ({error}); waiting for the close to be reported"
                );
            }
        }
    }
    let release = result.thinking_changed == Some(false) && reply.is_some();
    if !(result.yield_turn || release) {
        return effects;
    }
    let finalize_prompt = reply.clone();
    let finalize = async {
        flush_audio(context.gemini, &mut context.media.audio_bytes).await?;
        if context.turns_candidate_open()
            && let Some(prompt) = reply.as_deref()
        {
            send_model_context(
                context.gemini,
                context.state,
                ModelInputKind::Turn,
                prompt,
                None,
            )
            .await?;
            context.activity.defer_thinking_reply(prompt);
            if !context.activity.discarding_output {
                context.activity.arm_thinking_reply_fallback(at, prompt);
            }
            *reply = None;
            context.state.clear_thinking_debt();
        }
        context.gemini.end_audio_turn().await
    }
    .await;
    if let Err(error) = finalize {
        eprintln!(
            "Gemini turn finalization failed ({error}); waiting for the close to be reported"
        );
        context
            .activity
            .mark_prompted(Instant::now(), finalize_prompt.as_deref(), false);
        effects.abandoned = true;
    }
    effects
}

/// Ends the interview by handing the loop the packet the browser would send.
///
/// Three routes reach the same ending now -- the candidate's own button, the
/// server-side deadline, and the interviewer's `end_interview` tool -- and the
/// last two arrive at the first one's path rather than at a copy of it, because
/// that is where the wrap-up, the report and the teardown are, and where they
/// are already tested. The result is always `Break` and the report has been
/// published by the time this returns, so there is nothing for a caller to do
/// but stop -- which holds because both callers check `ended` first. Called on
/// an interview that has already ended it would return having published
/// nothing, and the caller would stop just the same.
async fn end_through_control(
    room: &Room,
    context: &mut GeminiEventContext<'_>,
    interview: InterviewContext<'_>,
    reason: &str,
    review: &mut SideCall,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // A review that came back between two watch ticks is a pause already paid
    // for, and the report is about to be written. Collected here rather than
    // left to the tick that will not run, because the alternative is the drop
    // below aborting a result that had already arrived.
    if let Some(finished) = review.finished()
        && let Ok(notes) = finished.await
    {
        record_interim_notes(context.state, &notes);
    }
    let _ = handle_data_packet(
        room,
        context,
        interview,
        TOPIC_CONTROL,
        &serde_json::json!({ "type": "end_interview", "reason": reason }),
        false,
    )
    .await?;
    Ok(())
}

/// Applies one decoded data packet. `Break` means the interview is over and the
/// report has been published.
async fn handle_data_packet(
    room: &Room,
    context: &mut GeminiEventContext<'_>,
    interview: InterviewContext<'_>,
    topic: &str,
    payload: &serde_json::Value,
    received: bool,
) -> Result<ControlFlow<()>, Box<dyn std::error::Error + Send + Sync>> {
    if settles_phase_judge(context.state, topic, payload) {
        settle_phase_judge(context, interview).await;
    }
    let judge_running = context.activity.phase_judge.is_running();
    if defers_round_transition(context.state, judge_running, topic, payload)
        && (judge_running || context.activity.reserve_phase_judge(Instant::now()))
    {
        if !judge_running {
            let judgment = spawn_phase_judge(context.state, interview);
            context.activity.phase_judge.start(judgment);
        }
        // A repeat of the same request keeps the first one's hold.
        context
            .activity
            .pending_round_transition
            .get_or_insert_with(|| ParkedPacket {
                payload: payload.clone(),
                received,
                deadline: Instant::now() + PHASE_JUDGE_SETTLE,
            });
        return Ok(ControlFlow::Continue(()));
    }
    apply_data_packet(room, context, interview, topic, payload, received).await
}

/// Only a coding drawing handover may replace the camera with pending ink.
/// Keep this decision beside its effects so invalid or unrelated controls
/// cannot consume queued images or disable the candidate's camera.
async fn settle_example_handover(
    yield_turn: bool,
    surface: Option<&str>,
    state: &mut RuntimeState,
    board: &mut board::Board,
    media: &mut media::CandidateMedia,
    gemini: &mut GeminiLiveSession,
) {
    if yield_turn
        && surface == Some("example")
        && !state.interview_mode.is_whiteboard()
        && !state.behavioral_round_started
    {
        board::resend_for_handover(board, state, gemini).await;
        if !board.allows_camera(true) {
            media.video = None;
        }
    }
}

/// Applies a packet after any judgment wait has been settled. Released round
/// transitions enter here so new speech or a slow judge cannot renew the hold.
async fn apply_data_packet(
    room: &Room,
    context: &mut GeminiEventContext<'_>,
    interview: InterviewContext<'_>,
    topic: &str,
    payload: &serde_json::Value,
    received: bool,
) -> Result<ControlFlow<()>, Box<dyn std::error::Error + Send + Sync>> {
    let resume_debt = ResumeDebt::capture(context.state);

    // One reading per packet, shared by every entry the packet produces.
    let receipt_timestamp_ms = crate::current_epoch_millis();
    let packet_at = Instant::now();
    let was_requested = context.state.thinking_hold.is_requested();
    let mut result = if received {
        apply_data_event_at(
            context.state,
            topic,
            payload,
            context.activity.last_test_reaction.elapsed().as_secs_f64(),
            receipt_timestamp_ms,
        )
    } else {
        crate::agent::apply_server_event_at(
            context.state,
            topic,
            payload,
            context.activity.last_test_reaction.elapsed().as_secs_f64(),
            receipt_timestamp_ms,
        )
    };
    if result.update_last_code_change {
        context.activity.last_code_change = Instant::now();
    }
    if result.update_last_test_reaction {
        context.activity.last_test_reaction = Instant::now();
    }
    if result.update_last_interjection {
        context.activity.last_interjection = Instant::now();
    }
    context
        .activity
        .settle_thinking_request(was_requested, context.state, &result);

    // Before the reply goes to the model, so a tick the packet earned reaches
    // the page without waiting on that write.
    session::flush_framework_progress(room, context.state).await;
    let mut reply = result.generate_reply.take();
    settle_example_handover(
        result.yield_turn,
        payload.get("surface").and_then(serde_json::Value::as_str),
        context.state,
        context.board,
        context.media,
        context.gemini,
    )
    .await;
    let grace = thinking_transcript_grace(interview.boot.silence_ms);
    let effects = settle_hold(context, &result, &mut reply, packet_at, grace).await;
    if effects.cut_off {
        close_turns(room, context).await?;
        set_agent_state(room, context.agent_state, AGENT_STATE_LISTENING).await?;
    }

    // The reply is already marked owed. Everything else the packet changed
    // still has to reach the page: a round it started cannot be started again.
    if effects.abandoned {
        reply = None;
    }

    // Whether what this packet asked the model to read reached it, which is
    // what settles a reminder of open earlier steps written into it.
    let mut delivered = false;
    if let Some(held) = result.held_context.take() {
        let held = crate::agent::with_timer(context.state, held);
        match send_model_context(
            context.gemini,
            context.state,
            ModelInputKind::Turn,
            &held,
            None,
        )
        .await
        {
            Ok(()) => delivered = true,
            Err(error) => eprintln!(
                "Gemini held context failed ({error}); waiting for the close to be reported"
            ),
        }
    }
    if let Some(paused) = result.pause_changed {
        room.local_participant()
            .publish_data(browser_packet(
                TOPIC_CONTROL,
                &serde_json::json!({ "type": "pause_state", "paused": paused }),
            )?)
            .await?;
        update_pause_activity(
            context.state,
            context.activity,
            context.output_audio,
            Instant::now(),
        );
        if paused {
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

    // Every result the browser reports, reacted to or not: one inside the
    // reaction cooldown updates the record without Jim saying anything, and a
    // later prompt that describes it is otherwise unexplained.
    if topic == crate::runtime::TOPIC_TEST_RESULTS {
        let judged = TestRunLine {
            run: context.state.last_test_run.as_ref(),
            note: result.test_run,
            credited_run_is_current: crate::agent::tested_code_is_current(context.state),
            reacted: reply.is_some(),
            ended: context.state.ended,
        };
        eprintln!(
            "tests: at={} {judged} room={}",
            log_clock(context.state),
            interview.boot.room_name
        );
    }
    if let Some(prompt) = reply {
        // Not `?`: a failed write here ended the interview with no report, and
        // the socket it failed on is replaced when the close is reported.
        let sent = send_model_text(
            context.gemini,
            context.state,
            ModelInputKind::Turn,
            TurnCause::Turn,
            &prompt,
        )
        .await;
        match record_event_prompt(
            context.state,
            context.activity,
            &prompt,
            resume_debt
                .as_ref()
                .filter(|_| result.pause_changed == Some(false)),
            sent,
            Instant::now(),
        ) {
            Ok(()) => {
                delivered = true;
                if result.carries_thinking_debt {
                    context.state.clear_thinking_debt();
                }

                // Which data event made Jim speak, against the progress it was
                // sent with, so a transcript that repeats a step can be matched
                // to the prompt that asked for it. After the send, so it
                // records a prompt that reached the socket.
                eprintln!(
                    "{}",
                    prompt_line(
                        context.state,
                        context.activity,
                        &format!("topic={topic}"),
                        interview.boot.room_name,
                    )
                );
            }
            Err(error) => {
                // A failed resume is owed by `record_event_prompt`; the debt it
                // carried is still in the state, unpaid, for the next socket.
                if result.carries_thinking_debt && result.pause_changed != Some(false) {
                    context
                        .activity
                        .mark_prompted(Instant::now(), Some(&prompt), false);
                }
                eprintln!(
                    "Gemini reply request failed ({error}); waiting for the close to be reported"
                );
            }
        }
    }
    context.state.settle_earlier_steps(delivered);
    let Some(reason) = result.finish_interview else {
        return Ok(ControlFlow::Continue(()));
    };
    if let Some(slot) = interview.slot {
        slot.assessment_finished();
    }

    session::flush_thinking_notice(room, context.state).await;

    // The assessment ends here, before the goodbye: the reducer has closed the
    // unasked steps of a round that never opened, and the turns still open are
    // the candidate's last words. The report is written from this point while
    // the farewell plays, so the candidate waits for the longer of the two
    // rather than for both. The farewell stays out of what is assessed: all it
    // can add is the skips a started behavioral round leaves to it, and the
    // report scores those steps as unassessed with or without them.
    //
    // Not `?`, and neither is anything else between here and
    // `publish_with_recovery`. The turns go out as a LiveKit text stream and
    // the report as a data packet, so the one failing says nothing about the
    // other, and a final segment the panel never saw is cosmetic where a
    // missing report is not.
    if let Err(error) = close_turns(room, context).await {
        eprintln!("closing the last turns failed ({error}); writing the report anyway");
    }

    // The board the browser sent just before ending can still be on its way;
    // see `BOARD_FINAL_WAIT`. Copied out rather than borrowed: the farewell
    // below holds the context, board and all, for as long as the report call
    // runs beside it, and a regeneration grades the same boards. Phase
    // checkpoints preserve work the candidate cleared before the final board.
    context.board.settle_for_report(context.state).await;
    let assessment = freeze_assessment(
        interview.boot,
        context.state,
        interview.started_at.elapsed().as_secs_f64() / 60.0,
        context.board.report_boards(),
    );
    let report_boards = assessment.report_boards();
    let mut recovery_events = interview.events.lock().await;
    let api_key = &**interview.keys;
    let farewell = async {
        // The goodbye is the only part of the ending that needs the Live
        // socket. The report is written over HTTP from what this process
        // already holds, so a socket that died under the goodbye costs the
        // goodbye and nothing else; propagating here dropped the report call
        // and left the candidate waiting on a report nobody was writing.
        if should_send_wrap_up(&reason)
            && let Err(error) = send_wrap_up_and_wait(room, context, &reason).await
        {
            eprintln!("wrap-up failed ({error}); writing the report without it");
        }

        // The goodbye is the last turn there will be. Closing here keeps the
        // panel from leaving it rendered as still-in-progress for the rest of
        // the page's life, and costs one publish.
        if let Err(error) = close_turns(room, context).await {
            eprintln!("closing the goodbye failed ({error}); writing the report anyway");
        }
    };
    let (generated, ()) = tokio::join!(
        generate_report_bounded(
            interview.boot,
            &assessment.prompt,
            &report_boards,
            (
                assessment.behavioral_round_opened(),
                assessment.drawing_received()
            ),
            api_key,
            crate::gemini::GENERATION_SEED,
            &assessment.refused,
        ),
        farewell
    );
    context.media.audio = None;
    context.media.video = None;
    context.media.audio_bytes.clear();
    let close_live = async {
        if let Err(error) = context.gemini.shutdown().await {
            eprintln!("Gemini close failed ({error}); leaving anyway");
        }
    };
    report::publish_with_recovery(
        report::ReportRecovery {
            boot: interview.boot,
            assessment,
            reason: &reason,
            keys: api_key,
            candidate: interview.candidate_identity,
        },
        generated,
        &mut recovery_events,
        room,
        close_live,
    )
    .await?;

    // Every report went out through a receipt wait, so leaving now drops
    // nothing the candidate has not acknowledged or stopped listening for.
    leave_room(room).await;
    Ok(ControlFlow::Break(()))
}

#[cfg(test)]
#[path = "../tests/unit/livekit.rs"]
mod tests;

#[cfg(test)]
#[path = "../tests/unit/livekit/cost.rs"]
mod cost_tests;
