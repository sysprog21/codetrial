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
    CANDIDATE_SPEAKER, INTERVIEWER_SPEAKER, ModelInputKind, RuntimeState, SpeakerTurn,
    framework_evidence_json, framework_progress, phase_id, read_editor_text,
    record_framework_evidence, released_follow_ups, unrecorded_earlier_phases, with_timer, wrap_up,
};
use crate::gemini::{GeminiEvent, GeminiFunctionCall, GeminiLiveSession};
use crate::runtime::{
    TOOL_END_INTERVIEW, TOOL_LOG_HINT, TOOL_READ_EDITOR, TOOL_RECORD_FRAMEWORK_EVIDENCE,
    TOPIC_CONTROL, TOPIC_TRANSCRIPTION,
};

use super::media::OutputAudio;
use super::turn::{Floor, Interruptible, RuntimeActivity, SpeakerTurns, TurnState, closing_order};
use super::{
    AGENT_STATE_LISTENING, AGENT_STATE_SPEAKING, LIVEKIT_AGENT_STATE, NOTABLE_PLAYOUT_BACKLOG,
    WRAP_UP_WAIT, browser_packet, output_settled,
};

/// The door every text sent over the live socket goes through, so that what
/// the session spent is measured where it is spent rather than at whichever
/// call sites somebody remembered. Tool answers have the other door, on their
/// way out of [`execute_tool_call`], which the behaviour harness drives without
/// a socket. The two fields are taken separately rather than as a
/// context, because the watch loop, the greeting and the cold restart each hold
/// a different pair.
pub(super) async fn send_model_text(
    gemini: &mut GeminiLiveSession,
    state: &mut RuntimeState,
    kind: ModelInputKind,
    text: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Counted before the write, and kept counted if the write fails. The
    // measurement is of the input this server constructed, which is a fact
    // about the session either way; a socket that dies mid-send is the
    // transport's business and the note above `EvidenceMetrics` says these
    // numbers are not about transport.
    state.evidence_ledger.record_model_input(kind, text);
    gemini.send_text(text).await
}

pub(super) struct GeminiEventContext<'a> {
    pub(super) output_audio: &'a mut OutputAudio,
    pub(super) gemini: &'a mut GeminiLiveSession,
    pub(super) state: &'a mut RuntimeState,
    pub(super) agent_state: &'a mut String,
    pub(super) activity: &'a mut RuntimeActivity,
    turns: &'a mut SpeakerTurns,
    candidate_identity: Option<&'a str>,
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

/// Split out of `handle_gemini_event` because it is the whole of what that
/// function decides before dispatching, and none of it needs a room, a socket
/// or an await. It is also the rule a restart has to get right: the discard
/// belongs to the socket that armed it, and a replacement that inherits one
/// drops its own first turn, which is the cold-restart briefing.
fn output_disposition(event: &GeminiEvent, discarding: bool, paused: bool) -> OutputDisposition {
    let is_output = matches!(
        event,
        GeminiEvent::Audio { .. } | GeminiEvent::OutputTranscript(_)
    );
    let ends_turn = matches!(event, GeminiEvent::TurnComplete | GeminiEvent::Interrupted);

    if discarding {
        if is_output {
            return OutputDisposition::Drop;
        }
        if ends_turn {
            return OutputDisposition::EndsTheDiscard;
        }
    }

    // Checked after the discard, not before it: a turn ending while paused
    // still has to serve the discard's sentence, and `TurnComplete` is not
    // output so it was never the thing a pause silences.
    if paused && is_output {
        return OutputDisposition::Drop;
    }
    OutputDisposition::Deliver
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
    match output_disposition(
        &event,
        context.activity.discarding_output,
        context.state.paused,
    ) {
        OutputDisposition::Drop => return Ok(()),
        OutputDisposition::EndsTheDiscard => context.activity.discarding_output = false,
        OutputDisposition::Deliver => {}
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
        GeminiEvent::TurnComplete => on_turn_complete(room, context).await,
        GeminiEvent::Interrupted => on_interruption(room, context).await,

        // Named rather than left to the catch-all: the room loop intercepts
        // this before dispatching, so the only way one arrives here is through
        // `send_wrap_up_and_wait`, where the interview ends within
        // `WRAP_UP_WAIT` and there is no socket left to replace.
        GeminiEvent::GoAway { .. } => Ok(()),
        _ => Ok(()),
    }
}

/// Answers every call in the batch, and republishes the checklist when one of
/// them moved it.
async fn on_tool_calls(
    room: &Room,
    context: &mut GeminiEventContext<'_>,
    calls: Vec<GeminiFunctionCall>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    for call in calls {
        let shown_before = framework_progress(context.state);
        let response = execute_tool_call(context.state, &call);
        context.gemini.send_tool_response(&call, response).await?;

        // Gemini now owes a generation for this, and will deliver it on this
        // socket or not at all.
        context.activity.tool_response_outstanding = true;
        if checklist_changed(&shown_before, context.state) {
            publish_framework_progress(room, context.state).await?;
        }
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
        context.activity.note_candidate_finished(Instant::now());
        let turn = &mut context.turns.candidate;
        let whole = turn
            .record(&mut context.state.transcript, CANDIDATE_SPEAKER, text)
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
            context.activity.awaiting_reply_since = None;
            eprintln!(
                "timing: {:.2}s from the candidate finishing to the reply starting",
                since.elapsed().as_secs_f64()
            );
        }
        context.activity.mark_speaking();

        // Speech is queued, not played: the floor stays busy until the buffered
        // audio actually finishes.
        context.activity.last_agent_speech = context.output_audio.playout_deadline;
        set_agent_state(room, context.agent_state, AGENT_STATE_SPEAKING).await?;
    }
    Ok(())
}

/// Gemini finished the turn. The room has not: the queue is still draining.
async fn on_turn_complete(
    room: &Room,
    context: &mut GeminiEventContext<'_>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Whatever the tool response was owed has now arrived.
    context.activity.tool_response_outstanding = false;

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
    context.activity.floor = Floor::AwaitingPlayout;
    if !context.output_audio.is_playing() {
        context.activity.mark_listening();
        set_agent_state(room, context.agent_state, AGENT_STATE_LISTENING).await?;
    }
    Ok(())
}

/// Gemini cut its own turn, because it heard the candidate start one.
async fn on_interruption(
    room: &Room,
    context: &mut GeminiEventContext<'_>,
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

    // The only other path that empties the queue, and it used to do so
    // silently. If a turn is cut this way the candidate hears a fragment or
    // nothing, and without this line the log shows only the consequence: a turn
    // that completed with nothing left to play.
    let unplayed = cut_off_turn(context.activity, context.output_audio);

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
    let response = tool_response(state, call);

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

fn tool_response(state: &mut RuntimeState, call: &GeminiFunctionCall) -> serde_json::Value {
    match call.name.as_str() {
        TOOL_READ_EDITOR => serde_json::json!({
            "result": read_editor_text(
                &state.language,
                &state.code,
                state.last_test_run.as_ref(),
                state.test_runs,
                crate::agent::minutes_left(state),
            )
        }),

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
            // the ones `read_editor` answers with.
            if requested {
                result.push_str("\n\n");
                result.push_str(&read_editor_text(
                    &state.language,
                    &state.code,
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
                Ok(evidence) => {
                    let mut response =
                        serde_json::json!({ "result": framework_evidence_json(&evidence) });
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
        // Gated on the same trusted evidence the behavioral round opens on, and
        // for the same reason: this is the model judging that its own interview
        // is finished, and the cost of believing it wrongly is a candidate cut
        // off partway. A two-round interview also has to reach the reserved
        // round's explicit started-or-skipped disposition. Refusing costs
        // nothing -- the timer still ends the session, which is what happened
        // before this tool existed -- so the gate is on the claim, not on the
        // clock.
        TOOL_END_INTERVIEW => {
            if !crate::agent::coding_round_complete(state) {
                return serde_json::json!({
                    "error": "The coding round has no Test and Optimizations evidence yet, so the interview is not finished. Continue, and record evidence when the candidate earns it."
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

pub(super) async fn send_wrap_up_and_wait(
    room: &Room,
    context: &mut GeminiEventContext<'_>,
    reason: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let farewell = with_timer(context.state, wrap_up(reason));
    send_model_text(
        context.gemini,
        context.state,
        ModelInputKind::Turn,
        &farewell,
    )
    .await?;
    context.activity.mark_speaking();
    let deadline = Instant::now() + WRAP_UP_WAIT;
    loop {
        if Instant::now() >= deadline {
            return Ok(());
        }
        // The closing message is the one turn that plays to the end.
        if output_settled(context.activity.floor, context.output_audio.is_playing()) {
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

    // The generation this was waiting for died with the turn.
    activity.tool_response_outstanding = false;
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
#[path = "../../tests/unit/livekit/session.rs"]
mod tests;
