//! The Gemini side of a running interview: the inbound event handlers, the tool
//! dispatch they call, the transcript publishing, and the procedures that end a
//! turn.
//!
//! Split out of `livekit.rs` along the line that file's own shape already drew.
//! The parent owns the room -- joining it, the select loop, the deadlines, the
//! browser's data packets -- and they meet only at [`GeminiEventContext`],
//! which is the borrow the select loop hands over for the length of one event.
//! That is the seam, rather than a scroll position.
//!
//! Not every model-bound text is here. The greeting, the cold restart, the
//! watch nudge and the interim prompt are built by the loop that decides to
//! send them and go out from the parent, through [`send_model_text`], which is
//! here because counting them is this half's job.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use ::livekit::data_stream::api::StreamTextOptions;
use ::livekit::prelude::Room;

use crate::agent::{
    CANDIDATE_SPEAKER, INTERVIEWER_SPEAKER, ModelInputKind, RuntimeState, SpeakerTurn, TestRunNote,
    framework_progress, phase_id, read_board_text, read_editor_text, record_framework_evidence,
    released_follow_ups, unrecorded_earlier_phases, with_timer, wrap_up,
};
use crate::gemini::{GeminiEvent, GeminiFunctionCall, GeminiLiveSession};
use crate::runtime::{
    TOOL_END_INTERVIEW, TOOL_LOG_HINT, TOOL_READ_BOARD, TOOL_READ_EDITOR,
    TOOL_RECORD_FRAMEWORK_EVIDENCE, TOPIC_CONTROL, TOPIC_TRANSCRIPTION,
};

use super::board::{self, Board};
use super::media::{CandidateMedia, OutputAudio};
use super::turn::{Floor, Interruptible, RuntimeActivity, SpeakerTurns, TurnState, closing_order};
use super::{
    AGENT_STATE_LISTENING, AGENT_STATE_SPEAKING, AGENT_STATE_THINKING, LIVEKIT_AGENT_STATE,
    NOTABLE_PLAYOUT_BACKLOG, WRAP_UP_WAIT, browser_packet, output_settled,
    pause_leaves_output_in_flight,
};

/// The door every realtime-input text goes through, so that what the session
/// spent is measured where it is spent rather than at whichever call sites
/// somebody remembered. The system instruction is counted when its Live socket
/// opens, and tool answers have the other door, on their way out of
/// [`execute_tool_call`], which the behaviour harness drives without a socket.
/// The two fields are taken separately rather than as a context, because the
/// watch loop, the greeting and the cold restart each hold a different pair.
pub(super) async fn send_model_text(
    gemini: &mut GeminiLiveSession,
    state: &mut RuntimeState,
    kind: ModelInputKind,
    cause: TurnCause,
    text: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Counted before the write, and kept counted if the write fails. The
    // measurement is of the input this server constructed, which is a fact
    // about the session either way; a socket that dies mid-send is the
    // transport's business and the note above `EvidenceMetrics` says these
    // numbers are not about transport.
    state.evidence_ledger.record_model_input(kind, text);
    gemini.input_cause = Some(cause.label());
    gemini.send_text(text).await
}

/// What asked the Live model for the generation a usage line bills, named on
/// that line so tokens can be traced to their source. Passed with each send
/// that asks for a reply, so no caller has to relabel one afterwards.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TurnCause {
    /// Greeting, test reaction, wrap-up and similar stage directions.
    Turn,
    Watch,
    Tool,
    Recovery,
}

impl TurnCause {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Turn => "turn",
            Self::Watch => "watch",
            Self::Tool => "tool",
            Self::Recovery => "recovery",
        }
    }
}

/// `send_model_text` for ordered context rather than realtime text: counted
/// the same way, sent as a `clientContent` turn that asks for a reply only
/// when `reply` names what asked. Context that asks for none starts no
/// generation, so it leaves the cause to whatever does.
pub(super) async fn send_model_context(
    gemini: &mut GeminiLiveSession,
    state: &mut RuntimeState,
    kind: ModelInputKind,
    text: &str,
    reply: Option<TurnCause>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    state.evidence_ledger.record_model_input(kind, text);
    if let Some(cause) = reply {
        gemini.input_cause = Some(cause.label());
    }
    gemini.send_context(text, reply.is_some()).await
}

pub(super) struct GeminiEventContext<'a> {
    pub(super) output_audio: &'a mut OutputAudio,
    pub(super) gemini: &'a mut GeminiLiveSession,
    /// The latest board, for the two paths that have to put it back in front
    /// of the model: `read_board`, and a session that came up remembering
    /// nothing.
    pub(super) board: &'a mut Board,
    pub(super) state: &'a mut RuntimeState,
    pub(super) agent_state: &'a mut String,
    pub(super) activity: &'a mut RuntimeActivity,
    pub(super) turns: &'a mut SpeakerTurns,
    candidate_identity: Option<&'a str>,
    pub(super) candidate_audio: &'a mut Vec<u8>,
}

impl GeminiEventContext<'_> {
    pub(super) fn turns_candidate_open(&self) -> bool {
        self.turns.candidate.is_open()
    }
}

/// What to do with an inbound Gemini event before its own arm sees it.
///
/// The two ways a reply is unwanted, and they are not the same. A pause is a
/// standing condition: it silences output for as long as it lasts and nothing
/// about the event changes it. A discard is one turn's sentence, armed when a
/// pause cut a reply already in flight, and the turn's own end is what serves
/// it -- which is why `Deliver` is not the whole answer here and
/// `EndsTheDiscard` exists.
#[derive(Debug, PartialEq, Eq)]
enum OutputDisposition {
    Deliver,
    Drop,
    EndsTheDiscard,
}

/// Time since the interview started, on every line a repeated-step timeline
/// is read from, so the server log lines up with the interview timer a
/// candidate reports against. To the millisecond, because a test result, a
/// `GoAway` and a replacement can all land inside one second, and their order
/// is the question.
pub(super) fn log_clock(state: &RuntimeState) -> String {
    clock(state.started_at.elapsed())
}

/// `m:ss.mmm`, split from the reading of the clock so a test can pin it.
pub(super) fn clock(elapsed: Duration) -> String {
    let seconds = elapsed.as_secs();
    format!(
        "{}:{:02}.{:03}",
        seconds / 60,
        seconds % 60,
        elapsed.subsec_millis()
    )
}

/// What a `tests:` log line reports about one result: the counts, whether it
/// earned credit and how it compares, whether the credited run still covers
/// the editor, and whether a reaction was asked for. `None` is a packet
/// dropped before it was judged, during a pause, in the behavioral round, or
/// one that ran nothing; the counts on record are then the previous run's, so
/// none are printed.
pub(super) struct TestRunLine<'a> {
    pub(super) run: Option<&'a serde_json::Value>,
    pub(super) note: Option<TestRunNote>,
    pub(super) credited_run_is_current: bool,
    pub(super) reacted: bool,
    pub(super) ended: bool,
}

impl std::fmt::Display for TestRunLine<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let Some(note) = self.note else {
            return f.write_str("outcome=dropped");
        };
        let count = |key| {
            self.run
                .and_then(|run| run.get(key))
                .and_then(serde_json::Value::as_i64)
                .unwrap_or(0)
        };
        let outcome = if self.reacted {
            "reaction_requested"
        } else if self.ended {
            "ended"
        } else {
            "cooldown"
        };
        write!(
            f,
            "passed={} total={} credited={} credited_run_is_current={} outcome={outcome}",
            count("passed"),
            count("total"),
            note.credited
                .map_or_else(|| "no".to_string(), |since| format!("{since:?}")),
            self.credited_run_is_current,
        )
    }
}

/// The `prompt:` line for a prompt that reached the socket, `source` naming
/// what sent it.
pub(super) fn prompt_line(
    state: &RuntimeState,
    activity: &RuntimeActivity,
    source: &str,
    room: &str,
) -> String {
    format!(
        "prompt: at={} {source} {} room={room}",
        log_clock(state),
        prompt_fields(state, activity)
    )
}

/// The shared fields of every `prompt:` log line, after the caller's label:
/// its number, and the progress it was sent against.
pub(super) fn prompt_fields(state: &RuntimeState, activity: &RuntimeActivity) -> String {
    format!(
        "id={} test_runs={} evidenced={}",
        activity.prompt_sequence,
        state.test_runs,
        framework_progress(state).join(",")
    )
}

/// Whether an event is Gemini actually answering what it was prompted with.
///
/// Not every output event is. A `TurnComplete` or `Interrupted` can belong to
/// the generation before the prompt, and empty audio and blank text say
/// nothing; letting any of those settle a prompt leaves no reply
/// owed when the socket is replaced before the real answer.
pub(super) fn answers_prompt(event: &GeminiEvent) -> bool {
    match event {
        GeminiEvent::Audio { bytes, .. } => !bytes.is_empty(),
        GeminiEvent::ToolCall(_) => true,
        GeminiEvent::Text(text) | GeminiEvent::OutputTranscript(text) => !text.trim().is_empty(),
        GeminiEvent::UsageRecorded
        | GeminiEvent::InputTranscript(_)
        | GeminiEvent::TurnComplete
        | GeminiEvent::Interrupted
        | GeminiEvent::GoAway { .. } => false,
    }
}

/// Split out of `handle_gemini_event` because it is the whole of what that
/// function decides before dispatching, and none of it needs a room, a socket
/// or an await. It is also the rule a restart has to get right: the discard
/// belongs to the socket that armed it, and a replacement that inherits one
/// drops its own first turn, which is the cold-restart briefing.
/// Speech or text that reaches the candidate, which a pause, a hold or a
/// discard silences.
fn is_output(event: &GeminiEvent) -> bool {
    matches!(
        event,
        GeminiEvent::Audio { .. } | GeminiEvent::OutputTranscript(_) | GeminiEvent::Text(_)
    )
}

fn output_disposition(event: &GeminiEvent, discarding: bool, held: bool) -> OutputDisposition {
    let is_output = is_output(event);
    let ends_turn = ends_turn(event);

    if discarding {
        if is_output {
            return OutputDisposition::Drop;
        }
        if ends_turn {
            return OutputDisposition::EndsTheDiscard;
        }
    }

    // Checked after the discard, not before it: a turn ending while held still
    // has to serve the discard's sentence, and `TurnComplete` is not output so
    // it was never the thing a pause or a hold silences.
    if held && is_output {
        return OutputDisposition::Drop;
    }
    OutputDisposition::Deliver
}

/// Logs and sums every usage observation the socket recorded and the room
/// never reached, for a socket being let go.
pub(super) fn drain_live_usage(room: &Room, context: &mut GeminiEventContext<'_>) {
    for usage in context.gemini.drain_usage() {
        record_live_usage(room, context, usage);
    }
}

fn record_live_usage(
    room: &Room,
    context: &mut GeminiEventContext<'_>,
    usage: crate::gemini::TokenUsage,
) {
    let line = context.activity.account_live_usage(
        room.name().as_str(),
        &super::log_clock(context.state),
        context.gemini.input_cause,
        context.state.context_compression,
        usage,
    );
    eprintln!("{line}");
}

/// A turn ending, whichever way it ends.
pub(super) fn ends_turn(event: &GeminiEvent) -> bool {
    matches!(event, GeminiEvent::TurnComplete | GeminiEvent::Interrupted)
}

/// The old turn was already cut off at pause. Its delayed ending must not
/// settle or cancel a prompt, candidate turn, or tool continuation sent since.
///
/// A `TurnComplete` the interruption guard swallows ends the discard too. With
/// both armed they wait on the same old turn, which sends one ending, and a
/// discard left behind it eats the next reply.
pub(super) fn accept_gemini_event(
    event: &GeminiEvent,
    activity: &mut RuntimeActivity,
    held: bool,
    now: Instant,
) -> bool {
    if activity.stale_turn_pending && ends_turn(event) {
        // An `Interrupted` is followed by the same turn's `TurnComplete`, which
        // ends the wait.
        activity.stale_turn_pending = matches!(event, GeminiEvent::Interrupted);
        return false;
    }
    if matches!(event, GeminiEvent::Interrupted) {
        activity.interrupted_turn_pending = true;
        activity.turn_completed_at = None;
    } else if answers_prompt(event) && !activity.discarding_output && !held {
        activity.interrupted_turn_pending = false;
        activity.stale_turn_pending = false;
    }
    if matches!(event, GeminiEvent::TurnComplete)
        && std::mem::take(&mut activity.interrupted_turn_pending)
    {
        activity.end_discard(false);
        return false;
    }
    match output_disposition(event, activity.discarding_output, held) {
        OutputDisposition::Drop => false,
        OutputDisposition::EndsTheDiscard => {
            activity.end_discard(matches!(event, GeminiEvent::Interrupted));
            false
        }
        OutputDisposition::Deliver => {
            // Old tool calls still need responses, but their generation cannot
            // answer the resume prompt or refresh its progress clock.
            if !activity.discarding_output {
                if answers_prompt(event) {
                    activity.note_output(now);
                } else if ends_turn(event) {
                    let said_something = activity.generating;
                    activity.note_turn_boundary(now);
                    if said_something && matches!(event, GeminiEvent::TurnComplete) {
                        activity.turn_completed_at = Some(now);
                    }
                }
            }
            true
        }
    }
}

/// Gemini said something. One arm each, because the arms share only the socket
/// they arrived on: what a tool call has to do and what a cut-off turn has to
/// undo have no step in common, and reading either one used to mean scrolling
/// past the other five.
pub(super) async fn handle_gemini_event(
    room: &Room,
    context: &mut GeminiEventContext<'_>,
    event: GeminiEvent,
    interruptible: Interruptible,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Read before the event settles anything: a completion arriving while a
    // reply is owed and nothing was produced is Gemini choosing silence.
    let silent_answer = matches!(event, GeminiEvent::TurnComplete)
        && !context.activity.generating
        && context.activity.owes_reply();

    // Every completion ends a generation the provider billed and whose context
    // it may have cut, including the late one an interruption or a pause leaves
    // behind and the guard below swallows.
    if matches!(event, GeminiEvent::TurnComplete) {
        context
            .activity
            .observe_turn_complete(context.state.context_compression);
    }
    let held = context.state.floor_held();
    let discarding = context.activity.discarding_output;
    let handled = if accept_gemini_event(&event, context.activity, held, Instant::now()) {
        if silent_answer && !context.activity.owes_reply() {
            eprintln!(
                "timing: Gemini completed a turn with no output; the owed reply is taken as a deliberate silence at={} room={}",
                log_clock(context.state),
                room.name()
            );
        }

        // However a turn ends, what asked for it has had its answer; after a
        // barge-in the next generation answers the candidate. Cleared at the
        // boundary rather than on a usage frame, which need not carry it. Only
        // a delivered ending: one swallowed above belongs to a generation
        // already cut off, and the cause of the prompt sent since is pending.
        if ends_turn(&event) {
            context.gemini.input_cause = None;
        }
        match event {
            GeminiEvent::ToolCall(calls) => on_tool_calls(room, context, calls).await,
            GeminiEvent::OutputTranscript(text) => on_output_transcript(room, context, &text).await,
            GeminiEvent::InputTranscript(text) => {
                on_input_transcript(room, context, &text, interruptible).await
            }
            GeminiEvent::Audio { bytes, mime_type } => {
                on_generated_audio(room, context, &bytes, &mime_type, interruptible).await
            }
            GeminiEvent::UsageRecorded => {
                if let Some(usage) = context.gemini.take_usage() {
                    record_live_usage(room, context, usage);
                }
                Ok(())
            }
            GeminiEvent::TurnComplete => on_turn_complete(room, context).await,
            GeminiEvent::Interrupted => on_interruption(room, context, interruptible).await,

            // Named rather than left to the catch-all: the room loop intercepts
            // this before dispatching, so the only way one arrives here is
            // through `send_wrap_up_and_wait`, where the interview ends within
            // `WRAP_UP_WAIT` and there is no socket left to replace.
            GeminiEvent::GoAway { .. } => Ok(()),
            _ => Ok(()),
        }
    } else {
        if is_output(&event) && (discarding || held) {
            drop_output(context.state, context.activity);
        } else if matches!(event, GeminiEvent::TurnComplete) {
            // An ending not dispatched still ends the utterance a spoken
            // request for thinking time waits on.
            settle_hold_at_turn_end(context.state, context.activity);
        }
        Ok(())
    };

    // The reply a hold deferred is due once its discard has ended, which is
    // often the swallowed ending above, so it is asked here on both paths.
    if handled.is_ok() && context.activity.claim_thinking_reply(context.state) {
        let prompt =
            crate::agent::with_timer(context.state, context.activity.thinking_reply_prompt());
        context
            .activity
            .mark_prompted(Instant::now(), Some(&prompt), false);
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
    }
    handled
}

/// What a turn's end settles about a hold: a spoken request whose utterance
/// has ended stands, and a candidate holding the floor is owed no reply.
pub(super) fn settle_hold_at_turn_end(state: &mut RuntimeState, activity: &mut RuntimeActivity) {
    activity.confirm_thinking_request(state, Instant::now(), crate::current_epoch_millis());
    if state.thinking_hold.is_active() {
        activity.awaiting_reply_since = None;
    }
}

/// Answers every call in the batch in one message, and republishes the
/// checklist once when any of them moved it.
async fn on_tool_calls(
    room: &Room,
    context: &mut GeminiEventContext<'_>,
    calls: Vec<GeminiFunctionCall>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if calls.is_empty() {
        return Ok(());
    }
    let shown_before = framework_progress(context.state);
    let answers = calls
        .into_iter()
        .map(|call| {
            let response = execute_tool_call(context.state, &call);

            // The phase as the checklist spells it, and only when it is one:
            // the argument is model text, and the rest of the call stays out of
            // the log for the reason the checklist carries no summary.
            if call.name == TOOL_RECORD_FRAMEWORK_EVIDENCE {
                let phase = call
                    .args
                    .get("phase")
                    .and_then(serde_json::Value::as_str)
                    .filter(|phase| {
                        crate::agent::REACTO_PHASE_IDS.contains(phase)
                            || crate::agent::STAR_PHASE_IDS.contains(phase)
                    })
                    .unwrap_or("?");
                eprintln!(
                    "evidence: at={} phase={phase} accepted={} room={}",
                    log_clock(context.state),
                    response.get("error").is_none(),
                    room.name()
                );
            }
            (call, response)
        })
        .collect::<Vec<_>>();

    // Not `?`. A response that did not go out is owed nothing back, so the flag
    // stays down, and the socket it failed on is replaced when its close is
    // reported; the checklist below still reflects the calls.
    match context.gemini.send_tool_responses(&answers).await {
        // Gemini now owes a generation for this, and will deliver it on this
        // socket or not at all.
        Ok(()) => {
            context.gemini.input_cause = Some(TurnCause::Tool.label());
            context.activity.note_tool_response(Instant::now());
        }
        Err(error) => {
            eprintln!(
                "Gemini tool response failed ({error}); waiting for the close to be reported"
            );
        }
    }
    if checklist_changed(&shown_before, context.state) {
        publish_framework_progress(room, context.state).await?;
    }

    // What `read_board` could not put in its own response. After the responses
    // rather than between them, so a batch that asked twice puts the board up
    // once, and after the text so the model reads what it is looking at before
    // it looks.
    //
    // Not `?`: an image the socket would not take is worth a line and a turn
    // that answers from the board it already had, not an interview ended on the
    // write.
    if std::mem::take(&mut context.state.board_resend_requested)
        && let Err(error) = board::resend(context.board, context.gemini).await
    {
        eprintln!("board resend failed ({error}); waiting for the close to be reported");
    }
    Ok(())
}

/// What the interviewer said, as it is said.
async fn on_output_transcript(
    room: &Room,
    context: &mut GeminiEventContext<'_>,
    text: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // `text` is passed whole, not the trimmed form: fragments have to be
    // concatenated exactly as received. The trimmed view only decides whether
    // this event carried anything at all.
    if transcript_text(text).is_some() {
        let turn = &mut context.turns.interviewer;
        let whole = turn
            .record(&mut context.state.transcript, INTERVIEWER_SPEAKER, text)
            .to_string();
        publish_transcript(room, &whole, turn.segment_id("interviewer"), false, None).await?;
    }
    Ok(())
}

/// What the candidate said, and the playout it cuts short.
async fn on_input_transcript(
    room: &Room,
    context: &mut GeminiEventContext<'_>,
    text: &str,
    interruptible: Interruptible,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if let (Some(_), Some(identity)) = (transcript_text(text), context.candidate_identity) {
        // The candidate is talking over audio Gemini finished producing a while
        // ago. Gemini will not call this an interruption, because as far as it
        // is concerned that turn ended when it stopped generating; only this
        // side knows the queue is still draining. Cut it here or the reply
        // lands behind the rest of the old turn.
        drop_stale_playout(room, context, interruptible).await?;
        context
            .activity
            .note_candidate_finished(Instant::now(), context.output_audio.is_playing());

        // The wait the room was shown is over once the candidate talks again:
        // the floor is theirs, and the watch tick shows it again if their new
        // turn goes unanswered too.
        if context.agent_state.as_str() == AGENT_STATE_THINKING {
            set_agent_state(room, context.agent_state, AGENT_STATE_LISTENING).await?;
        }
        let whole = context
            .turns
            .candidate
            .record(&mut context.state.transcript, CANDIDATE_SPEAKER, text)
            .to_string();
        let was_thinking = context.state.thinking_hold.is_active();
        let was_requested = context.state.thinking_hold.is_requested();
        if interruptible == Interruptible::Yes {
            context.activity.observe_thinking_fragment(
                context.state,
                &whole,
                Instant::now(),
                crate::current_epoch_millis(),
            );
        }
        if !was_thinking && context.state.thinking_hold.is_active() {
            context.state.thinking_unheard_reply |= context.activity.floor != Floor::Listening;
            cut_off_for_hold(context.activity, context.output_audio);
            set_agent_state(room, context.agent_state, AGENT_STATE_LISTENING).await?;
        } else if was_thinking
            && !context.state.thinking_hold.is_active()
            && crate::agent::thinking_owes_context(context.state)
        {
            let briefing = speech_release_briefing(context.state, was_requested);
            let briefing = crate::agent::with_timer(context.state, briefing);
            match send_model_context(
                context.gemini,
                context.state,
                ModelInputKind::Turn,
                &briefing,
                None,
            )
            .await
            {
                Ok(()) => context.state.clear_thinking_debt(),
                Err(error) => {
                    context
                        .activity
                        .mark_prompted(Instant::now(), Some(&briefing), false);
                    eprintln!(
                        "Gemini thinking context failed ({error}); waiting for the close to be reported"
                    );
                }
            }
        }
        publish_transcript(
            room,
            &whole,
            context.turns.candidate.segment_id("candidate"),
            false,
            Some(identity),
        )
        .await?;
    }
    Ok(())
}

/// A chunk of the interviewer's voice, queued for the room.
async fn on_generated_audio(
    room: &Room,
    context: &mut GeminiEventContext<'_>,
    bytes: &[u8],
    mime_type: &str,
    interruptible: Interruptible,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Read before the interrupt below, because that is the point the candidate
    // stops waiting. Stamped on the last input transcript fragment, so it
    // covers endpointing plus model latency plus anything still queued ahead of
    // the reply: the silence the candidate actually sits through.
    //
    // `None` means Gemini is answering something it never transcribed, and
    // there is no moment the candidate finished to measure from. Printing
    // anything then is worse than printing nothing.
    let waited = context.activity.awaiting_reply_since;

    // A new turn's first chunk while the previous one is still draining.
    // `InputTranscript` normally clears the queue before this point, so
    // reaching here means Gemini answered something it never transcribed.
    // Backstop rather than the main path, and it must run before `capture` or
    // the new audio queues behind the old.
    //
    // Only for a chunk that will actually be queued. Dropping ahead of a chunk
    // `capture` rejects leaves the candidate with a sentence cut in half and no
    // reply behind it.
    if context.output_audio.accepts(bytes, mime_type) {
        drop_stale_playout(room, context, interruptible).await?;
    }
    if context.output_audio.capture(bytes, mime_type).await? {
        // Cleared here rather than where it is read: a chunk `capture` rejects
        // is not the reply starting, and consuming the stamp on one would lose
        // the measurement for the chunk that is.
        if let Some(since) = waited {
            eprintln!(
                "timing: {:.2}s from the candidate finishing to the reply starting",
                since.elapsed().as_secs_f64()
            );
        }
        context.activity.note_reply_audible();

        // Speech is queued, not played: the floor stays busy until the buffered
        // audio actually finishes.
        context.activity.last_agent_speech = context.output_audio.playout_deadline;
        set_agent_state(room, context.agent_state, AGENT_STATE_SPEAKING).await?;
    }
    Ok(())
}

/// What speech that ends a hold tells the model, as context: whatever the hold
/// left owed, and for a declared hold that the candidate is ready. A withdrawn
/// request was reasoning that began like one, so it says nothing about being
/// ready; only the debt its brief hold ran up.
fn speech_release_briefing(state: &RuntimeState, withdrawn: bool) -> String {
    if withdrawn {
        crate::agent::owed_context(state)
    } else {
        crate::agent::thinking_resume(state)
    }
}

/// Once a held reply has been dropped, releasing the hold must not play its
/// tail; its own boundary disarms the discard. The model still holds what it
/// said, so the release has to disown it. A pause drops output too, but a
/// resumed interview gets its own line and owes nothing for it here.
fn drop_output(state: &mut RuntimeState, activity: &mut RuntimeActivity) {
    if state.thinking_hold.is_active() {
        activity.discarding_output = true;
        state.thinking_unheard_reply = true;
    }
}

/// Agent to browser, so no generated fixture covers it; `receiveControl` in
/// `web/interview.js` is the consumer, and both sides test this exact shape.
pub(super) fn thinking_state_message(thinking: bool) -> serde_json::Value {
    serde_json::json!({"type": "thinking_state", "thinking": thinking})
}

/// Tells the page what `take_thinking_notice` holds, if anything. Not `?`: a
/// page left showing the wrong button is corrected by the next click, and is no
/// reason to end the interview.
pub(super) async fn flush_thinking_notice(room: &Room, state: &mut RuntimeState) {
    if let Some(thinking) = state.take_thinking_notice()
        && let Err(error) = publish_thinking_state(room, thinking).await
    {
        eprintln!("publishing thinking state failed ({error}); continuing the interview");
    }
}

async fn publish_thinking_state(
    room: &Room,
    thinking: bool,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    room.local_participant()
        .publish_data(browser_packet(
            TOPIC_CONTROL,
            &thinking_state_message(thinking),
        )?)
        .await?;
    Ok(())
}

/// Gemini finished the turn. The room has not: the queue is still draining.
async fn on_turn_complete(
    room: &Room,
    context: &mut GeminiEventContext<'_>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    settle_hold_at_turn_end(context.state, context.activity);

    // The gap between Gemini finishing and the queue emptying. Gemini
    // synthesises far faster than speech plays, so this is how long the agent
    // will still be talking after it has stopped thinking, and therefore how
    // long a candidate answering now would have waited before
    // `drop_stale_playout` existed.
    let backlog = context
        .output_audio
        .playout_deadline
        .saturating_duration_since(Instant::now());

    // Only a backlog a candidate would notice. `!is_zero()` fired on a
    // millisecond and printed "0.0s", so every one of these lines in a real
    // session said nothing at all.
    if backlog >= NOTABLE_PLAYOUT_BACKLOG {
        eprintln!(
            "timing: turn generated, {:.1}s of it still to play",
            backlog.as_secs_f64()
        );
    }

    // Gemini finishing its turn also means the candidate utterance it answered
    // is over, so both sides close here.
    close_turns(room, context).await?;
    context
        .activity
        .settle_completed_turn(context.output_audio.is_playing());
    if context.activity.floor == Floor::Listening {
        set_agent_state(room, context.agent_state, AGENT_STATE_LISTENING).await?;
    }
    Ok(())
}

/// Gemini cut its own turn, because it heard the candidate start one.
async fn on_interruption(
    room: &Room,
    context: &mut GeminiEventContext<'_>,
    interruptible: Interruptible,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // A barge-in cancels a pending close. `cut_off_turn` below clears
    // `tool_response_outstanding`, which is the only thing holding the end
    // back, so without this the candidate who says "wait, actually" over Jim's
    // acknowledgement clears the hold with the same event that carries their
    // objection, and is cut off and handed a report. Ending is the one decision
    // here nobody can take back, and someone who has just started a sentence is
    // not finished. Jim can call the tool again once they are.
    if std::mem::take(&mut context.state.end_requested) {
        eprintln!("interviewer's ending cancelled: the candidate spoke over the close");
    }

    // A kept turn is over as far as Gemini is concerned: it has stopped
    // generating. Its later `TurnComplete` is ignored by accept_gemini_event,
    // so the floor is settled the way a finished one settles it here or the
    // loop waits for a reply that has already happened. Why a turn is kept at
    // all is on `cut_unless_protected`, and the line `on_turn_complete` prints
    // carries how much of it is still to play.
    let Some(unplayed) = cut_unless_protected(
        &context.state.transcript,
        interruptible,
        context.activity,
        context.output_audio,
    ) else {
        eprintln!(
            "timing: kept a turn Gemini cut: {}",
            if interruptible == Interruptible::No {
                "the closing turn plays to the end"
            } else {
                "the candidate has said nothing yet"
            }
        );
        return on_turn_complete(room, context).await;
    };

    // What Gemini heard is the whole diagnosis. It interrupts on its own voice
    // activity detection, so a cut with the candidate mid-sentence is barge-in
    // working. A cut with nothing transcribed is usually speaker echo or room
    // noise, and naming those makes the log actionable without pretending the
    // server can tell them apart.
    let heard = context.turns.candidate.tail(80);
    eprintln!(
        "timing: Gemini cut its own turn, {:.1}s of it unplayed; candidate audio so far: {}",
        unplayed.as_secs_f64(),
        if heard.is_empty() {
            "(nothing transcribed; check speaker echo or background noise)"
        } else {
            heard
        }
    );

    // A cut-off turn is still over. Without this the next thing either party
    // says appends to the abandoned turn under its segment id, so the panel
    // would glue two separate utterances into one row and the report prompt
    // would read them as one line.
    close_turns(room, context).await?;

    set_agent_state(room, context.agent_state, AGENT_STATE_LISTENING).await?;
    Ok(())
}

pub(super) async fn set_agent_state(
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

/// How long Gemini waits through the candidate's silence before taking the
/// turn, so the page can draw the countdown to it. An attribute rather than a
/// message: it is fixed for the interview, and a page that joins or reloads
/// after the agent reads it without asking. `TURN_WINDOW_ATTRIBUTE` in
/// `web/lib.js` names the same key.
pub(super) const TURN_WINDOW_ATTRIBUTE: &str = "codetrial.silence_ms";

/// The agent's first attributes, listening and the turn window, in the one
/// round trip the room pays before it waits for the candidate.
pub(super) async fn publish_opening_attributes(
    room: &Room,
    current: &mut String,
    silence_ms: u32,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let participant = room.local_participant();
    let attributes = agent_state_attributes(participant.attributes(), AGENT_STATE_LISTENING);
    participant
        .set_attributes(turn_window_attributes(attributes, silence_ms))
        .await?;
    current.clear();
    current.push_str(AGENT_STATE_LISTENING);
    Ok(())
}

fn turn_window_attributes(
    mut attributes: HashMap<String, String>,
    silence_ms: u32,
) -> HashMap<String, String> {
    attributes.insert(TURN_WINDOW_ATTRIBUTE.to_string(), silence_ms.to_string());
    attributes
}

fn agent_state_attributes(
    mut attributes: HashMap<String, String>,
    state: &str,
) -> HashMap<String, String> {
    attributes.insert(LIVEKIT_AGENT_STATE.to_string(), state.to_string());
    attributes
}

/// Public so the behaviour check in `tests/interview_behavior.rs` answers a
/// text model's tool calls with this dispatch rather than a copy of it.
pub fn execute_tool_call(state: &mut RuntimeState, call: &GeminiFunctionCall) -> serde_json::Value {
    let mut response = tool_response(state, call);

    // Only under an explicit compression window, where the dialogue before a
    // tool call can leave the context while the model waits on the answer.
    // Without one this is text every later turn is billed for again.
    if state.context_compression.is_some()
        && !state.ended
        && !state.end_requested
        && [
            TOOL_READ_EDITOR,
            TOOL_READ_BOARD,
            TOOL_LOG_HINT,
            TOOL_RECORD_FRAMEWORK_EVIDENCE,
        ]
        .contains(&call.name.as_str())
    {
        response["turn_context"] = serde_json::Value::String(crate::agent::with_timer(
            state,
            crate::agent::editor_tool_continuity(state),
        ));
    }

    // Counted here, once, on the way out, rather than in the arms. Every tool
    // answer is text handed back to the model, and counting only `read_editor`
    // left the hint, the evidence and the end-of-interview replies out of what
    // the session was said to cost. The arms below return early on refusals, so
    // a count taken inside them would have missed exactly those. What is
    // measured is the serialized response, which the socket then carries inside
    // the function-response envelope around it.
    let kind = if call.name == TOOL_READ_EDITOR {
        ModelInputKind::ReadEditor
    } else {
        ModelInputKind::ToolResponse
    };
    state
        .evidence_ledger
        .record_model_input(kind, &response.to_string());
    response
}

/// What the model is told when it tries to hint or end the interview while the
/// candidate holds the floor to think. Both would speak into a silence the
/// candidate asked for.
const REFUSED_DURING_HOLD: &str =
    "The candidate requested thinking time. Stay silent until they speak again or yield the turn.";

fn tool_response(state: &mut RuntimeState, call: &GeminiFunctionCall) -> serde_json::Value {
    if state.thinking_hold.is_active()
        && matches!(call.name.as_str(), TOOL_END_INTERVIEW | TOOL_LOG_HINT)
    {
        return serde_json::json!({ "error": REFUSED_DURING_HOLD });
    }
    match call.name.as_str() {
        // The image cannot travel in this response, so the tool records the ask
        // and the room loop answers it with a realtime image; see
        // `board::resend`. The text is what a picture cannot say: how much is
        // on the board, and how long ago it was drawn.
        TOOL_READ_BOARD => {
            state.board_resend_requested = true;
            serde_json::json!({
                "result": read_board_text(
                    state.board_strokes,
                    state.board_snapshots,
                    crate::agent::board_age_seconds(state),
                    crate::agent::minutes_left(state),
                )
            })
        }

        TOOL_READ_EDITOR => {
            state.code_shown = state.code.clone();
            let from_line = call
                .args
                .get("fromLine")
                .and_then(serde_json::Value::as_u64)
                .map_or(1, |line| usize::try_from(line).unwrap_or(usize::MAX));
            serde_json::json!({
                "result": read_editor_text(
                    &state.language,
                    &state.code,
                    from_line,
                    state.last_test_run.as_ref(),
                    state.test_runs,
                    crate::agent::minutes_left(state),
                )
            })
        }

        // Missing reads as asked for: the declaration requires the flag, and
        // the cost of the other default is a hint the candidate asked for
        // arriving without its rung.
        TOOL_LOG_HINT => {
            let requested = call
                .args
                .get("requested")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(true);
            let mut result = crate::agent::record_hint(state, requested);

            // The editor comes with the clue, so a requested hint is one tool
            // call rather than `read_editor` and then this: the model is told
            // to fit the clue to their code, and asking for the code first was
            // a whole round trip before it could say anything. The fences are
            // the ones `read_editor` answers with. A whiteboard has no editor
            // to fence, so the board comes instead, the way `read_board` sends
            // it: the newest board can still be inside the send interval, and a
            // clue fitted to the one before it is fitted to work the candidate
            // has already moved past.
            if requested && state.interview_mode.is_whiteboard() {
                state.board_resend_requested = true;
            } else if requested {
                state.code_shown = state.code.clone();
                result.push_str("\n\n");
                result.push_str(&read_editor_text(
                    &state.language,
                    &state.code,
                    1,
                    state.last_test_run.as_ref(),
                    state.test_runs,
                    crate::agent::minutes_left(state),
                ));
            }
            serde_json::json!({ "result": result })
        }

        // The call that completes the coding round also hands over the
        // follow-ups, once: a later Test or Optimizations note finds the round
        // already complete and returns the evidence alone.
        TOOL_RECORD_FRAMEWORK_EVIDENCE => {
            let was_complete = crate::agent::coding_round_complete(state);
            let shown_before = framework_progress(state);
            match record_framework_evidence(state, &call.args) {
                // The phase alone. The row echoed back was the model's own
                // arguments plus a timestamp and a version, held in the session
                // for the rest of the interview on every call.
                Ok(evidence) => {
                    let mut response = serde_json::json!({
                        "result": format!("Recorded {}.", phase_id(evidence.phase))
                    });
                    if !was_complete && let Some(follow_ups) = released_follow_ups(state) {
                        response["followUps"] = follow_ups.into();
                    }

                    // Only on the call that first ticks this step: a second
                    // note on Coding is not a new gap, and asking again on
                    // every one would push the model toward inventing the
                    // earlier steps to make the reminder stop.
                    let phase = phase_id(evidence.phase);
                    let newly_shown = !shown_before.contains(&phase)
                        && framework_progress(state).contains(&phase);
                    if newly_shown
                        && let Some(reminder) = unrecorded_earlier_phases(state, &evidence)
                    {
                        response["earlierSteps"] = reminder.into();
                    }
                    response
                }
                Err(error) => serde_json::json!({ "error": error }),
            }
        }

        // A request, answered here, acted on by the room loop. Ending the
        // interview publishes a report and leaves the room, and none of that is
        // reachable from a function whose whole world is the state: what this
        // can do is say so, and be read on the way out of the event that
        // carried it.
        //
        // The response tells Jim to stay quiet because Gemini owes a generation
        // for every tool response, and the closing is about to be prompted for
        // properly. Without this it says goodbye twice.
        //
        // A coding-only session belongs to the candidate for its full duration.
        // REACTO evidence records work attempted, including failed tests; it
        // cannot authorize taking the remaining time away. Leave termination to
        // the candidate or the existing browser/server timer paths, even when
        // all tests pass. No second clock belongs in this tool.
        //
        // For a two-round session, gated on the evidence the behavioral round
        // opens on, and for the same reason: this is the model judging that its
        // own interview is finished, and the cost of believing it wrongly is a
        // candidate cut off partway. A two-round interview also has to reach
        // the reserved round's explicit started-or-skipped disposition.
        // Refusing costs nothing -- the timer still ends the session, which is
        // what happened before this tool existed -- so the gate is on the
        // claim, not on the clock.
        TOOL_END_INTERVIEW => {
            if state.interview_loop == crate::agent::InterviewLoop::CodingOnly
                || !crate::agent::coding_round_complete(state)
            {
                return serde_json::json!({
                    "error": crate::agent::end_interview_refusal(state)
                });
            }
            if state.interview_loop == crate::agent::InterviewLoop::CodingBehavioral
                && !state.round_transition_seen
            {
                return serde_json::json!({
                    "error": "The behavioral reserve has not started or been skipped yet, so the interview is not finished. Continue until its round transition arrives."
                });
            }
            state.end_requested = true;
            serde_json::json!({
                "result": "Recorded. Say nothing further; the closing will be requested in a moment."
            })
        }
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
                "phases": framework_progress(state),
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

/// The goodbye has been said and has played. Settled output alone is not
/// enough: the wrap-up can go out behind an acknowledgement that stalled and
/// was released, and that turn's late ending settles the floor before the
/// goodbye has begun. The goodbye's own prompt stays owed across it, since it
/// went out behind a turn, so it is waited on too.
pub(super) fn goodbye_heard(activity: &RuntimeActivity, audio_playing: bool) -> bool {
    output_settled(activity.floor, audio_playing) && !activity.owes_prompt()
}

pub(super) async fn send_wrap_up_and_wait(
    room: &Room,
    context: &mut GeminiEventContext<'_>,
    reason: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let farewell = with_timer(
        context.state,
        wrap_up(reason, context.state.behavioral_round_started),
    );
    send_model_text(
        context.gemini,
        context.state,
        ModelInputKind::Turn,
        TurnCause::Turn,
        &farewell,
    )
    .await?;
    context
        .activity
        .mark_prompted(Instant::now(), Some(&farewell), false);
    let deadline = Instant::now() + WRAP_UP_WAIT;
    loop {
        if Instant::now() >= deadline {
            return Ok(());
        }
        // The closing message is the one turn that plays to the end.
        if goodbye_heard(context.activity, context.output_audio.is_playing()) {
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

/// A hold took the floor while Jim was still talking: stop him, and drop the
/// rest of the turn he was in if Gemini is still producing it. A discard
/// already under way is kept, since its turn has not ended either. A
/// generation that had stalled arms no discard, as at a pause, but its late
/// ending is still kept from settling what the hold's release asks for.
pub(super) fn cut_off_for_hold(activity: &mut RuntimeActivity, output_audio: &mut OutputAudio) {
    activity.discarding_output |= pause_leaves_output_in_flight(activity, Instant::now());
    activity.stale_turn_pending |= activity.generating && !activity.discarding_output;
    cut_off_turn(activity, output_audio);
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
pub(super) fn cut_off_turn(
    activity: &mut RuntimeActivity,
    output_audio: &mut OutputAudio,
) -> Duration {
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

    activity.prompted_at = None;
    activity.prompt_behind_turn = false;
    activity.generating = false;
    activity.last_output_at = None;

    // The generation this was waiting for died with the turn.
    activity.tool_response_outstanding = false;
    unplayed
}

/// Cuts the queued turn, unless this turn has to play to the end or the
/// candidate has not been transcribed yet in this interview. Returns what it
/// threw away, or `None` when it kept it.
///
/// `Interruptible::No` is the closing message, which `send_wrap_up_and_wait`
/// waits out and `take_stale_playout` already refuses to drop. Gemini's own
/// interruption used to walk past that promise from the other side: the arm
/// that dispatches it had the mode in hand and did not pass it on, so a
/// candidate clearing their throat over the goodbye cut the goodbye.
///
/// Gemini interrupts on its own voice activity detection, and the opening
/// seconds of a room give it plenty that is not speech: a microphone opening,
/// a chair, a breath. Until the candidate has been recorded saying something
/// there is nobody who could be barging in, and the turn the cut would discard
/// is already generated and queued, so discarding it hands the candidate
/// exactly the silence this path exists to explain. Two consecutive interviews
/// lost ten seconds of the opening line that way, each with nothing
/// transcribed.
///
/// Asked of the transcript rather than of a flag kept beside it, because the
/// transcript is what "the candidate has spoken" means everywhere else here
/// and a second copy of that answer can disagree with it: `on_input_transcript`
/// admits any non-empty text, while a transcript line is opened only for
/// spoken words, so a fragment that is nothing but a hashtag artifact would
/// set the flag and record nothing. That fragment is the room noise this
/// exists to ignore.
///
/// The candidate's first words are the case this defers rather than serves. A
/// genuine barge-in over the greeting keeps playing until they are
/// transcribed, and `drop_stale_playout` cuts the queue when they are. Every
/// barge-in after that is immediate.
pub(super) fn cut_unless_protected(
    transcript: &[String],
    interruptible: Interruptible,
    activity: &mut RuntimeActivity,
    output_audio: &mut OutputAudio,
) -> Option<Duration> {
    let cut = interruptible == Interruptible::Yes && crate::agent::candidate_lines(transcript) > 0;
    cut.then(|| cut_off_turn(activity, output_audio))
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
pub(super) async fn close_turns(
    room: &Room,
    context: &mut GeminiEventContext<'_>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let candidate_identity = context.candidate_identity.map(str::to_string);

    let order = closing_order(
        context.turns.interviewer.transcript_line(),
        context.turns.candidate.transcript_line(),
    );
    let mut closing = [
        (
            "interviewer",
            close_turn(&mut context.turns.interviewer, "interviewer"),
        ),
        (
            "candidate",
            close_turn(&mut context.turns.candidate, "candidate"),
        ),
    ];
    let receipt_timestamp_ms = crate::current_epoch_millis();
    for speaker in order {
        let Some((whole, segment_id)) = closing
            .iter_mut()
            .find(|(name, _)| *name == speaker)
            .and_then(|(_, closed)| closed.take())
        else {
            continue;
        };
        context
            .state
            .evidence_ledger
            .record_conversation_turn(receipt_timestamp_ms, speaker);

        // Only the candidate's own speech is attributed to them; the
        // interviewer publishes as the agent participant.
        let identity = (speaker == "candidate")
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
    pub(super) fn context<'a>(
        &'a mut self,
        output_audio: &'a mut OutputAudio,
        gemini: &'a mut GeminiLiveSession,
        board: &'a mut Board,
        media: &'a mut CandidateMedia,
    ) -> GeminiEventContext<'a> {
        GeminiEventContext {
            output_audio,
            gemini,
            board,
            state: &mut self.state,
            agent_state: &mut self.agent_state,
            activity: &mut self.activity,
            turns: &mut self.turns,
            candidate_identity: media.identity.as_deref(),
            candidate_audio: &mut media.audio_bytes,
        }
    }
}

#[cfg(test)]
#[path = "../../tests/unit/livekit/session.rs"]
mod tests;
