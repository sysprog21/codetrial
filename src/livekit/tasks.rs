//! LiveKit task sessions retain the authorized package for their entire
//! lifetime.

use futures_util::StreamExt;
use serde_json::{Value, json};
use std::collections::VecDeque;
use tokio::time::{Duration, Instant};

use super::media::{
    CandidateMedia, discard_paused_audio, handle_media_event, publish_output_audio, pump_audio,
};
use super::{
    GEMINI_OUTPUT_AUDIO_SAMPLE_RATE, REPORT_TIMEOUT, Room, RoomEvent, browser_packet, join_room,
};
use crate::config::AgentConfig;
use crate::gemini::{GeminiEvent, GeminiKeys, GeminiLiveSession, TokenUsage};
use crate::runtime::{TOPIC_TASK_CONNECTED, TOPIC_TASK_ERROR, TOPIC_TASK_REVIEW, TOPIC_TASK_STATE};
use crate::tasks::access::PinnedTask;
use crate::tasks::default_number;
use crate::tasks::session::{
    Cause, ErrorMessage, Interviewer, MAX_CODE_BYTES, MAX_TURN_BYTES, Phase, TaskSession,
};

type Error = Box<dyn std::error::Error + Send + Sync>;

/// When the task room may open another Gemini socket. How many restarts a
/// session gets, and when a long-lived socket earns the count back, is the
/// interview's rule, `take_restart_attempt`; this adds only what a task room
/// does differently: a backoff between attempts, and deliberate restarts that
/// do not count against a failure.
struct RecoveryBudget {
    attempts: usize,
    opened_at: Instant,
    /// How long the socket that just failed had lived, until the first
    /// attempt to replace it spends it.
    failed_socket_age: Option<Duration>,
    outage_started: bool,
    retry_at: Instant,
    intentional_restart: bool,
}

impl RecoveryBudget {
    fn new(now: Instant) -> Self {
        Self {
            attempts: 0,
            opened_at: now,
            failed_socket_age: None,
            outage_started: false,
            retry_at: now,
            intentional_restart: false,
        }
    }

    fn failed(&mut self, now: Instant) {
        if !self.outage_started {
            self.failed_socket_age = Some(now.saturating_duration_since(self.opened_at));
            self.outage_started = true;
        }
    }

    fn exhausted(&self) -> bool {
        self.attempts >= super::GEMINI_RESTART_LIMIT && !self.intentional_restart
    }

    fn start_attempt(&mut self, now: Instant) -> bool {
        if now < self.retry_at {
            return false;
        }
        let allowed = if self.intentional_restart {
            self.attempts += 1;
            true
        } else {
            // Only the first attempt after a failure is measured against the
            // socket that failed; one that never opened lived for nothing.
            let age = self.failed_socket_age.take().unwrap_or_default();
            super::take_restart_attempt(&mut self.attempts, age)
        };
        if allowed {
            self.retry_at = now + super::COLD_OPEN_BACKOFF;
        }
        allowed
    }

    fn failed_attempt(&mut self, now: Instant) {
        self.intentional_restart = false;
        self.retry_at = now + super::COLD_OPEN_BACKOFF;
    }

    fn recovered(&mut self, now: Instant) {
        if self.intentional_restart {
            self.attempts = self.attempts.saturating_sub(1);
            self.intentional_restart = false;
        }
        self.opened_at = now;
        self.outage_started = false;
    }

    /// What the learner is told about Jim while no socket is open: nothing
    /// new during a deliberate restart, which discards speech queued while
    /// the learner was away rather than recovering from a fault, and Hint or
    /// Discuss sent
    /// meanwhile waits for the new socket. Only a failure is shown.
    fn outage_availability(&self) -> Option<Interviewer> {
        if self.intentional_restart {
            None
        } else if self.exhausted() {
            Some(Interviewer::Unavailable)
        } else {
            Some(Interviewer::Degraded)
        }
    }

    fn allow_immediate(&mut self, now: Instant) {
        self.retry_at = now;
        self.intentional_restart = true;
    }
}

/// Bytes of one task data-channel message. The largest is a revision whose
/// code is at `MAX_CODE_BYTES`, and JSON may spell one byte of it as six
/// (`\u0000`), so any code the session accepts fits.
const MAX_TASK_MESSAGE_BYTES: usize = 6 * MAX_CODE_BYTES + 4096;
/// Bytes of captured learner turns a replacement connection is briefed with.
const RECOVERY_TURN_BYTES: usize = 4096;
/// Bytes of the interviewer's last reply kept for that briefing.
const LAST_REPLY_BYTES: usize = 2048;

/// A task room's Live usage, logged in the interview's spelling so
/// `scripts/analyze-gemini-usage.py` prices both the same way.
struct LiveUsage {
    session: u64,
    started: Instant,
    socket: u64,
    events: u64,
    total: TokenUsage,
}

impl LiveUsage {
    fn new() -> Self {
        Self {
            session: crate::current_epoch_millis(),
            started: Instant::now(),
            socket: 1,
            events: 0,
            total: TokenUsage::default(),
        }
    }

    fn record(&mut self, room: &str, usage: TokenUsage) {
        self.events += 1;
        eprintln!(
            "{}",
            super::turn::live_turn_usage_line(
                room,
                self.session,
                &super::session::clock(self.started.elapsed()),
                self.socket,
                self.events,
                "task",
                &usage,
            )
        );
        self.total.add(usage);
    }

    /// Everything a socket recorded that the loop never took, for a socket
    /// being let go.
    fn drain(&mut self, room: &str, gemini: &mut GeminiLiveSession) {
        for usage in gemini.drain_usage() {
            self.record(room, usage);
        }
    }

    fn summary(&self, room: &str, model: &str, outcome: super::LiveOutcome) -> String {
        super::live_usage_line(
            room,
            self.session,
            model,
            self.started.elapsed().as_secs(),
            outcome,
            Some(self.socket),
            &self.total,
        )
    }
}

/// What a replacement connection needs beyond the task state to carry on the
/// conversation: the newest learner turns, the interviewer's last reply, and
/// any speech the dropped connection never answered. All of it is untrusted
/// learner-facing material, delimited as such.
fn recovery_conversation(session: &TaskSession, last_reply: &str, unanswered: &str) -> String {
    let turns: Vec<_> = session
        .recent_turns(RECOVERY_TURN_BYTES)
        .into_iter()
        .map(|(id, text)| json!({"turnId": id, "text": text}))
        .collect();
    let owed = if unanswered.trim().is_empty() {
        "No learner speech is waiting for a reply; wait for the learner.".to_owned()
    } else {
        format!(
            "The learner was speaking when the connection dropped and is owed a reply: {}",
            json!(unanswered.trim())
        )
    };
    format!(
        "Continue the same conversation; do not greet the learner again or repeat a question they answered. Possibly misrecognized lines support no check.\nBEGIN UNTRUSTED RECENT CONVERSATION\nlearnerTurns={}\nyourLastReply={}\nEND UNTRUSTED RECENT CONVERSATION\n{owed}",
        json!(turns),
        json!(last_reply)
    )
}

/// Puts `replacement` in place of `gemini`, shuts the old socket down and
/// counts its usage before it goes.
async fn retire_socket(
    gemini: &mut GeminiLiveSession,
    replacement: GeminiLiveSession,
    usage: &mut LiveUsage,
    room: &str,
) {
    let mut old = std::mem::replace(gemini, replacement);
    let _ = old.shutdown().await;
    usage.drain(room, &mut old);
    usage.socket += 1;
}

/// Tells a replacement socket where the task stands and what the dropped one
/// missed, then replays queued context. A reply owed for speech the old socket
/// never answered is asked for with the briefing, unless queued context is
/// about to ask for one anyway; without that, Jim stayed silent until the
/// learner spoke again.
async fn brief_replacement(
    gemini: &mut GeminiLiveSession,
    session: &TaskSession,
    last_reply: &str,
    unanswered: &str,
    pending: &mut VecDeque<String>,
) -> Result<(), Error> {
    let briefing = format!(
        "Authoritative task state: {}. Existing checks and hint support: {}. Issued support request IDs: {}. Already delivered conceptual hints: {}. Do not reset the deadline or repeat supplied hints.\n{}",
        session.snapshot(),
        json!(session.checks),
        session.support_context(),
        json!(&session.task.exercise.hints()[..session.hint_rungs_used]),
        recovery_conversation(session, last_reply, unanswered)
    );
    let owed = !unanswered.trim().is_empty() && pending.is_empty();
    gemini.send_context(&briefing, owed).await?;
    while let Some(context) = pending.pop_front() {
        if let Err(error) = gemini.send_text(&context).await {
            pending.push_front(context);
            return Err(error);
        }
    }
    Ok(())
}

/// A `task_error`, typed, so its shape is checked where it is written.
async fn publish_error(room: &Room, error: ErrorMessage) -> Result<(), Error> {
    publish(room, TOPIC_TASK_ERROR, &serde_json::to_value(error)?).await
}

async fn publish(room: &Room, topic: &str, value: &Value) -> Result<(), Error> {
    if let Ok(packet) = browser_packet(topic, value)
        && room.local_participant().publish_data(packet).await.is_err()
    {
        eprintln!("codetrial task_publish_degraded topic={topic}");
    }
    Ok(())
}

async fn send_wrap_up_question(
    gemini: &mut GeminiLiveSession,
    session: &mut TaskSession,
    id: &str,
    asked_at: &mut Option<u64>,
    now: u64,
) -> Result<(), Error> {
    let question = session.task.exercise.check_question(id).unwrap_or_default();
    let result = gemini
        .send_text(&format!(
            "Ask only this wrap-up question, then wait: {question}"
        ))
        .await;
    if result.is_err() {
        // Selection reserves a bounded opportunity; a failed send must not
        // spend it before the replacement socket can ask the question.
        session.retry_wrap_up_check(id);
    } else {
        *asked_at = Some(now);
    }
    result
}

fn wrap_up_should_stop(session: &TaskSession) -> bool {
    session.final_revision_id.is_some()
}

fn lost_room_review(session: &mut TaskSession, room: &str) -> Option<Value> {
    if matches!(session.phase, Phase::Ready | Phase::Feedback) {
        return None;
    }
    session.conclude(Cause::RoomLoss, 0, crate::current_epoch_seconds());
    Some(crate::tasks::report::ended(session, room))
}

fn silence_output(output: &mut super::media::OutputAudio, media: &mut CandidateMedia) {
    output.interrupt();
    discard_paused_audio(media);
}

/// Whether the room loop goes on after an event.
enum Flow {
    Continue,
    Stop,
}

/// One task room's state between events. The loop in `run` only waits; each
/// source of events has a method here, so no handler is buried in the
/// `select!`, where rustfmt cannot reach it.
struct TaskRoom<'a> {
    config: &'a AgentConfig,
    room: Room,
    room_name: &'a str,
    candidate: &'a str,
    keys: GeminiKeys,
    boot: TaskLive<'a>,
    gemini: GeminiLiveSession,
    output: super::media::OutputAudio,
    media: CandidateMedia,
    session: TaskSession,
    setup_deadline: Instant,
    /// Wall-clock second of the learner's latest transcribed speech.
    last_speech: u64,
    /// Wall-clock second the latest learner turn was captured.
    last_completed_turn: u64,
    turn: u64,
    /// Learner speech since the last completed turn.
    transcript: String,
    /// The interviewer's reply in progress, and the last completed one.
    reply: String,
    last_reply: String,
    suppress_output: bool,

    pending_context: VecDeque<String>,
    disconnected_at: Option<Instant>,
    /// When the pending wrap-up question was asked.
    wrap_up_asked_at: Option<u64>,
    alive: bool,
    recovery: RecoveryBudget,
    usage: LiveUsage,
    /// The sequence number last published, so an unchanged state is not sent
    /// again every second.
    published_seq: Option<u64>,
}

impl TaskRoom<'_> {
    /// Publishes the state when it changed since the last publication, or
    /// always when `force`, as for a learner who has just rejoined.
    async fn publish_state(&mut self, force: bool) -> Result<(), Error> {
        let seq = self.session.seq();
        if force || self.published_seq != Some(seq) {
            publish(&self.room, TOPIC_TASK_STATE, &self.session.snapshot()).await?;
            self.published_seq = Some(seq);
        }
        Ok(())
    }

    /// Sends `context` to the interviewer now, or keeps it for the next socket
    /// when none is open or the learner's page has left the room.
    async fn send_or_queue(&mut self, context: String) {
        if self.alive {
            if self.gemini.send_text(&context).await.is_err() {
                self.alive = false;
                self.pending_context.push_back(context);
            }
        } else {
            self.pending_context.push_back(context);
        }
    }

    async fn on_room_event(&mut self, event: RoomEvent) -> Result<Flow, Error> {
        if handle_media_event(
            &mut self.media,
            &mut self.gemini,
            self.candidate,
            false,
            &event,
        )
        .await
        .is_err()
        {
            self.alive = false;
        }
        match event {
            RoomEvent::ParticipantConnected(participant)
                if participant.identity().0 == self.candidate =>
            {
                self.disconnected_at = None;
                if self.session.page_returned() {
                    self.alive = false;
                    self.recovery.allow_immediate(Instant::now());
                }
                self.publish_state(true).await?;
            }
            RoomEvent::ParticipantDisconnected(participant)
                if participant.identity().0 == self.candidate =>
            {
                self.disconnected_at = Some(Instant::now());
                self.session.page_left();
                silence_output(&mut self.output, &mut self.media);
                self.suppress_output = true;
            }
            RoomEvent::Disconnected { .. } => return Ok(Flow::Stop),
            RoomEvent::DataReceived {
                payload,
                topic,
                participant,
                ..
            } if participant
                .as_ref()
                .is_some_and(|p| p.identity().0 == self.candidate) =>
            {
                self.on_data(topic.as_deref(), &payload).await?;
            }
            _ => {}
        }
        Ok(Flow::Continue)
    }

    /// Applies one learner message to the session. `None` means the message
    /// is not one this room answers, and is ignored without a reply; `Some`
    /// carries the result and any context for the interviewer.
    async fn on_data(&mut self, topic: Option<&str>, payload: &[u8]) -> Result<(), Error> {
        // A packet the page sent before it left can arrive at the deadline; the
        // loss is settled first, so it cannot complete the attempt.
        self.settle_page_loss(crate::current_epoch_seconds());
        if self.session.phase == Phase::Feedback {
            return Ok(());
        }
        if payload.len() > MAX_TASK_MESSAGE_BYTES {
            // Too large to parse for its request id, but answered, so the
            // page's capture does not wait out its timeout for nothing.
            return publish_error(
                &self.room,
                ErrorMessage::new(
                    "action_rejected",
                    None,
                    true,
                    "The message is too large; shorten the code and try again.",
                ),
            )
            .await;
        }
        let now = crate::current_epoch_seconds();
        if !self.alive
            && self.session.interviewer != Interviewer::Unavailable
            && let Some(state) = self.recovery.outage_availability()
        {
            self.session.availability(state);
        }
        let previous_phase = self.session.phase;
        let previous_target = self.session.active_target_id.clone();
        let Some(result) = self
            .session
            .receive(topic, payload, &self.boot.greeting, now)
        else {
            return Ok(());
        };

        match result {
            Ok(mut context) => {
                if previous_phase != self.session.phase
                    || previous_target != self.session.active_target_id
                {
                    context = Some(format!(
                        "{}\n{}",
                        self.session.provider_context(),
                        context.unwrap_or_default()
                    ));
                }

                // An attempt ended under a rule or by a failure gets no model
                // turn, not even a closing line.
                if let Some(context) = context.filter(|_| !self.session.ended_short()) {
                    self.send_or_queue(context).await;
                }

                // The handshake is answered with the state even when nothing in
                // it changed: that answer is how the page knows it is
                // connected.
                self.publish_state(topic == Some(TOPIC_TASK_CONNECTED))
                    .await?;
            }
            Err(_) => {
                let request_id = serde_json::from_slice::<Value>(payload)
                    .ok()
                    .and_then(|value| {
                        value["requestId"]
                            .as_str()
                            .filter(|id| crate::tasks::opaque_id(id))
                            .map(str::to_owned)
                    });
                publish_error(
                    &self.room,
                    ErrorMessage::new(
                        "action_rejected",
                        request_id,
                        true,
                        "Wait for the acknowledged task state before retrying.",
                    ),
                )
                .await?;
            }
        }
        Ok(())
    }

    async fn on_audio_frame(
        &mut self,
        frame: Option<::livekit::webrtc::audio_frame::AudioFrame<'static>>,
    ) {
        let ended = frame.is_none();
        if self.session.page_present() && self.session.phase != Phase::Ready && self.alive {
            if pump_audio(&mut self.media, &mut self.gemini, frame)
                .await
                .is_err()
            {
                self.alive = false;
            }
        } else {
            discard_paused_audio(&mut self.media);
        }
        if ended {
            self.media.audio = None;
        }
    }

    async fn on_gemini_event(&mut self, event: Option<GeminiEvent>) -> Result<(), Error> {
        // An attempt ended under a rule or by the page's loss says nothing more
        // to the model: a tool answer would only set it talking again.
        self.settle_page_loss(crate::current_epoch_seconds());
        if self.session.ended_short() {
            return Ok(());
        }
        let listening = self.session.page_present();
        match event {
            Some(GeminiEvent::Audio { bytes, mime_type }) if listening && !self.suppress_output => {
                if self.output.capture(&bytes, &mime_type).await.is_err() {
                    self.alive = false;
                }
            }
            Some(GeminiEvent::InputTranscript(text)) if listening => {
                self.last_speech = crate::current_epoch_seconds();
                if self.transcript.len() + text.len() <= MAX_TURN_BYTES {
                    self.transcript.push_str(&text);
                } else {
                    self.session.mark_overflow();
                }
            }
            Some(GeminiEvent::OutputTranscript(text)) => {
                if self.reply.len() + text.len() <= LAST_REPLY_BYTES {
                    self.reply.push_str(&text);
                }
            }
            Some(GeminiEvent::UsageRecorded) => {
                if let Some(observed) = self.gemini.take_usage() {
                    self.usage.record(self.room_name, observed);
                }
            }
            Some(GeminiEvent::TurnComplete) => self.on_turn_complete().await,
            Some(GeminiEvent::Interrupted) => self.output.interrupt(),
            Some(GeminiEvent::ToolCall(calls)) => {
                // A check recorded after the deadline is work done after time
                // ran out.
                self.session.expire(crate::current_epoch_seconds());
                if self.session.ended_short() {
                    return Ok(());
                }
                let answers: Vec<_> = calls
                    .into_iter()
                    .map(|call| {
                        let answer = self.session.answer_tool(&call.name, &call.args);
                        (call, answer)
                    })
                    .collect();
                if self.gemini.send_tool_responses(&answers).await.is_err() {
                    self.alive = false;
                }
                self.publish_state(false).await?;
            }
            Some(GeminiEvent::GoAway { .. }) | None => self.alive = false,
            _ => {}
        }
        Ok(())
    }

    async fn on_turn_complete(&mut self) {
        if !self.reply.trim().is_empty() {
            self.last_reply = std::mem::take(&mut self.reply);
        }
        if !self.transcript.trim().is_empty() {
            self.turn += 1;
            let id = format!("turn-{}", self.turn);
            let now = crate::current_epoch_seconds();
            self.session.observe_turn(&id, &self.transcript, now);
            self.last_completed_turn = now;
            if self.session.page_present()
                && self
                    .gemini
                    .send_context(
                        &format!(
                            "Captured learner turn ID: {id}; current acknowledged revision ID: {}. Only reliable engineering content can support a check.",
                            self.session.current.id
                        ),
                        false,
                    )
                    .await
                    .is_err()
            {
                self.alive = false;
            }
            self.transcript.clear();
        }
        self.suppress_output = false;
    }

    /// Ends the attempt as interrupted when the page is gone for good, before
    /// anything else can expire it.
    fn settle_page_loss(&mut self, now: u64) -> PageLoss {
        let rejoin = Duration::from_secs(default_number("pendingAdmissionSeconds"));
        let loss = page_loss(
            &self.session,
            self.disconnected_at.map(|at| at.elapsed()),
            rejoin,
        );
        if loss == PageLoss::Interrupted {
            self.session.conclude(Cause::Reload, 0, now);
        }
        loss
    }

    async fn on_tick(&mut self) -> Result<Flow, Error> {
        let now = crate::current_epoch_seconds();
        if self.settle_page_loss(now) == PageLoss::Stop {
            return Ok(Flow::Stop);
        }
        if self.session.wrap_up_open(now) && self.session.page_present() && self.alive {
            self.ask_wrap_up(now).await;
        }
        if self.session.phase == Phase::Ready && Instant::now() >= self.setup_deadline {
            return Ok(Flow::Stop);
        }
        let previous_phase = self.session.phase;
        if let Some(context) =
            self.session
                .tick(now, self.session.thinking, speaking(self.last_speech, now))
        {
            let changed = previous_phase != self.session.phase;
            let context = if changed {
                format!("{}\n{context}", self.session.provider_context())
            } else {
                context
            };
            if self.session.page_present() {
                self.send_or_queue(context).await;
            } else if changed {
                self.pending_context.push_back(context);
            }
        }

        // A learner thinking quietly sends Gemini nothing, and the reader gives
        // up on a socket silent for `READ_IDLE_LIMIT`; the pong keeps it.
        // Suspension keeps the socket too. A failed ping has already ended the
        // reader, and the close it found is what marks the socket dead.
        if self.alive
            && let Err(error) = self.gemini.keep_alive().await
        {
            eprintln!("Gemini ping failed ({error}); waiting for the close to be reported");
        }
        if !self.alive {
            self.recover().await;
        }
        self.publish_state(false).await?;
        Ok(Flow::Continue)
    }

    /// Asks the next uncovered check once the previous one was answered or
    /// timed out, and ends the task once there is nothing left to ask.
    async fn ask_wrap_up(&mut self, now: u64) {
        if self.session.thinking
            || !wrap_up_due(
                self.wrap_up_asked_at,
                self.last_completed_turn,
                self.last_speech,
                now,
            )
        {
            return;
        }
        match self.session.next_wrap_up_check() {
            Some(id) => {
                if send_wrap_up_question(
                    &mut self.gemini,
                    &mut self.session,
                    &id,
                    &mut self.wrap_up_asked_at,
                    now,
                )
                .await
                .is_err()
                {
                    self.alive = false;
                }
            }
            None if wrap_up_should_stop(&self.session) => self.session.feedback(now),
            None => {}
        }
    }

    /// Opens a replacement socket when the budget allows, briefs it with the
    /// task state and the conversation it missed, and replays queued context.
    async fn recover(&mut self) {
        self.recovery.failed(Instant::now());
        if let Some(state) = self.recovery.outage_availability() {
            self.session.availability(state);
        }
        silence_output(&mut self.output, &mut self.media);
        if self.session.phase == Phase::Feedback
            || !self.session.page_present()
            || !self.recovery.start_attempt(Instant::now())
        {
            return;
        }
        self.boot.instructions = self.session.provider_context();
        let opened = tokio::time::timeout(
            Duration::from_secs(5),
            crate::gemini::live_session_with_keys(&self.keys, &self.boot, None),
        )
        .await;
        let Ok(Ok(recovered)) = opened else {
            self.recovery.failed_attempt(Instant::now());
            return;
        };
        retire_socket(&mut self.gemini, recovered, &mut self.usage, self.room_name).await;

        // A reply cut off mid-sentence is what the learner heard last, so the
        // replacement is told that rather than the reply before it.
        let interrupted = std::mem::take(&mut self.reply);
        let heard = if interrupted.trim().is_empty() {
            self.last_reply.clone()
        } else {
            interrupted
        };
        if brief_replacement(
            &mut self.gemini,
            &self.session,
            &heard,
            &self.transcript,
            &mut self.pending_context,
        )
        .await
        .is_err()
        {
            self.recovery.failed_attempt(Instant::now());
            return;
        }
        self.alive = true;
        self.recovery.recovered(Instant::now());
        self.suppress_output = false;
        self.session.availability(Interviewer::Available);
    }

    /// Generates, retains and publishes the Learning review once the task
    /// reached feedback.
    async fn deliver_review(&mut self, slot: &crate::dispatch::Slot) -> Result<(), Error> {
        self.output.interrupt();
        let _ = self.gemini.shutdown().await;
        slot.assessment_finished();

        // An attempt that ended under a rule or a failure is recorded as that,
        // with nothing rated and no model asked.
        if self.session.ended_short() {
            let result = crate::tasks::report::ended(&self.session, self.room_name);
            self.session.task.retain_result(
                self.room_name,
                result.clone(),
                crate::current_epoch_seconds(),
            );
            return publish(&self.room, TOPIC_TASK_REVIEW, &result).await;
        }
        let exercise = &self.session.task.exercise;
        let (optimal, pitfalls) = exercise.reference_notes();
        let reference = format!(
            "{optimal}\n{pitfalls}\n{}",
            exercise.reference_code().unwrap_or("")
        );
        let review = tokio::time::timeout(
            REPORT_TIMEOUT,
            crate::gemini::generate_task_feedback(
                &self.keys,
                self.config.gemini_report_model.as_str(),
                &self.session,
                &reference,
                self.room_name,
            ),
        )
        .await
        .ok()
        .flatten()
        .unwrap_or_else(|| crate::tasks::report::unavailable(&self.session, self.room_name));
        self.session.task.retain_result(
            self.room_name,
            review.clone(),
            crate::current_epoch_seconds(),
        );
        publish(&self.room, TOPIC_TASK_REVIEW, &review).await
    }

    async fn close(mut self) -> Result<(), Error> {
        if let Some(review) = lost_room_review(&mut self.session, self.room_name) {
            self.session
                .task
                .retain_result(self.room_name, review, crate::current_epoch_seconds());
        }
        let _ = self.gemini.shutdown().await;
        self.usage.drain(self.room_name, &mut self.gemini);
        eprintln!(
            "{}",
            self.usage.summary(
                self.room_name,
                &self.boot.config.gemini_live_model,
                super::LiveOutcome::Ok,
            )
        );
        self.room.close().await?;
        Ok(())
    }
}

pub(crate) async fn run(
    config: &AgentConfig,
    room_name: &str,
    candidate: &str,
    task: PinnedTask,
    slot: &crate::dispatch::Slot,
) -> Result<(), Error> {
    let now = crate::current_epoch_seconds();
    let (room, mut events) =
        join_room(config, room_name, &format!("task-agent-{room_name}"), now).await?;
    let keys = GeminiKeys::from_config(config);
    let boot = TaskLive::new(config, &task);

    // Moves past a key Gemini rejects within the first-open limit, as an
    // interview's first open does.
    let mut gemini = match crate::gemini::check_live_session(&keys, &boot).await {
        Ok(gemini) => gemini,
        _ => {
            publish_error(
                &room,
                ErrorMessage::new(
                    "setup_failed",
                    None,
                    true,
                    "The task provider could not connect. Retry Start.",
                ),
            )
            .await?;
            let _ = room.close().await;
            return Err(std::io::Error::other("task provider setup failed").into());
        }
    };
    let output = match publish_output_audio(&room, GEMINI_OUTPUT_AUDIO_SAMPLE_RATE).await {
        Ok(output) => output,
        Err(_) => {
            publish_error(
                &room,
                ErrorMessage::new(
                    "setup_failed",
                    None,
                    true,
                    "The task audio could not connect. Retry Start.",
                ),
            )
            .await?;
            let _ = gemini.shutdown().await;
            let _ = room.close().await;
            return Err(std::io::Error::other("task audio setup failed").into());
        }
    };
    let mut session = TaskSession::new(task, now);
    let setup_deadline =
        Instant::now() + Duration::from_secs(default_number("pendingAdmissionSeconds"));
    let connected = room
        .remote_participants()
        .values()
        .any(|participant| participant.identity().0 == candidate);
    if !connected {
        session.page_left();
    }
    let mut state = TaskRoom {
        config,
        room,
        room_name,
        candidate,
        keys,
        boot,
        gemini,
        output,
        media: CandidateMedia::new(),
        session,
        setup_deadline,
        last_speech: now,
        last_completed_turn: now,
        turn: 0,
        transcript: String::new(),
        reply: String::new(),
        last_reply: String::new(),
        suppress_output: false,
        pending_context: VecDeque::new(),

        // A page that never joins is absent from the start, so its admission is
        // let go the same way as one that joined and left.
        disconnected_at: (!connected).then(Instant::now),
        wrap_up_asked_at: None,
        alive: true,
        recovery: RecoveryBudget::new(Instant::now()),
        usage: LiveUsage::new(),
        published_seq: None,
    };

    // Whatever ends the loop, an error included, `close` still runs, so an
    // attempt in progress is kept as interrupted rather than lost.
    let outcome = state.serve(&mut events, slot).await;
    let closed = state.close().await;
    outcome.and(closed)
}

impl TaskRoom<'_> {
    async fn serve(
        &mut self,
        events: &mut tokio::sync::mpsc::UnboundedReceiver<RoomEvent>,
        slot: &crate::dispatch::Slot,
    ) -> Result<(), Error> {
        let state = self;
        state.publish_state(true).await?;
        let mut interval = tokio::time::interval(Duration::from_secs(1));
        loop {
            let flow = tokio::select! {
                event = events.recv() => match event {
                    Some(event) => state.on_room_event(event).await?,
                    None => Flow::Stop,
                },
                frame = async { state.media.audio.as_mut().unwrap().next().await }, if state.media.audio.is_some() => {
                    state.on_audio_frame(frame).await;
                    Flow::Continue
                }
                event = state.gemini.next_event(), if state.alive => {
                    state.on_gemini_event(event).await?;
                    Flow::Continue
                }
                _ = interval.tick() => state.on_tick().await?,
            };
            if state.session.phase == Phase::Feedback {
                state.deliver_review(slot).await?;
                break;
            }
            if matches!(flow, Flow::Stop) {
                break;
            }
        }
        Ok(())
    }
}

/// What the task room's Live session is opened with: the operator's model,
/// voice and audio settings, and the task's own instructions and tools. Never
/// a catalog problem's notes or an interview plan.
pub(crate) struct TaskLive<'a> {
    config: &'a AgentConfig,
    pub(crate) instructions: String,
    pub(crate) greeting: String,
}

impl<'a> TaskLive<'a> {
    pub(crate) fn new(config: &'a AgentConfig, task: &PinnedTask) -> Self {
        Self {
            config,
            instructions: task
                .exercise
                .task_prompt_with_hint_limit(
                    Phase::Ready,
                    None,
                    task.exercise.starter(task.language()).unwrap_or(""),
                    task.language(),
                    task.hint_limit,
                )
                .expect("validated task prompt"),
            greeting: "Welcome. Ask the learner to explain their first reading of the task. Ask one question and wait; never provide completed code.".to_owned(),
        }
    }
}

impl crate::gemini::LiveSession for TaskLive<'_> {
    fn live_settings(&self) -> crate::gemini::LiveSettings<'_> {
        crate::gemini::LiveSettings {
            model: &self.config.gemini_live_model,
            voice: &self.config.gemini_voice,
            instructions: &self.instructions,
            tools: crate::gemini::task_tool_declarations(),
            vocabulary: Vec::new(),
            silence_ms: self.config.gemini_silence_ms,
            start_sensitivity: &self.config.gemini_start_sensitivity,
            end_sensitivity: self.config.gemini_end_sensitivity.as_deref(),
            // A camera reaches Gemini in no task: the page watches it locally.
            candidate_video: false,
            context_compression: self.config.gemini_context_compression,
        }
    }
}

/// Whether the next wrap-up question may be asked: the learner is not
/// speaking, and the last one was answered, or went unanswered too long, or
/// none was asked yet.
fn wrap_up_due(
    asked_at: Option<u64>,
    last_completed_turn: u64,
    last_speech: u64,
    now: u64,
) -> bool {
    if speaking(last_speech, now) {
        return false;
    }
    asked_at.is_none_or(|asked| {
        last_completed_turn > asked
            || now.saturating_sub(asked) >= default_number("wrapUpAnswerSeconds")
    })
}

/// Whether the learner spoke in the last few seconds; nudges and wrap-up
/// questions wait until they stop.
fn speaking(last_speech: u64, now: u64) -> bool {
    now.saturating_sub(last_speech) < 3
}

/// How long the agent waits, before Start, for a page that is not in the room.
const READY_ABSENCE: Duration = Duration::from_secs(10);

/// What a page that left the room means for the attempt.
#[derive(Debug, PartialEq)]
enum PageLoss {
    /// Nothing was started, so there is nothing to wait for: stopping frees
    /// the account for a fresh Start straight away.
    Stop,
    /// The page is gone and cannot resume: it held the attempt's monitoring,
    /// and nothing is persisted. (A deadline passing while it is gone is the
    /// session's to settle: `expire` knows the page is absent.)
    Interrupted,
    Wait,
}

fn page_loss(session: &TaskSession, gone_for: Option<Duration>, rejoin: Duration) -> PageLoss {
    let Some(gone_for) = gone_for else {
        return PageLoss::Wait;
    };
    match session.phase {
        // Long enough for a page to join after the agent, short enough that a
        // learner calibrating again finds the account free.
        Phase::Ready if gone_for >= READY_ABSENCE => PageLoss::Stop,
        Phase::Ready => PageLoss::Wait,
        Phase::Feedback => PageLoss::Wait,
        _ if gone_for >= rejoin => PageLoss::Interrupted,
        _ => PageLoss::Wait,
    }
}

#[cfg(test)]
#[path = "../../tests/unit/livekit/tasks.rs"]
mod tests;
