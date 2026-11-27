//! How an inbound Gemini event falls against the turn it belongs to, and what
//! accepting it does to the activity: the current turn, the late ending of one
//! a hold already cut off, or the tail of an interrupted one. The room loop and
//! the startup speech an early End preserves both judge it the same way.

use std::time::Instant;

use super::super::{RuntimeActivity, SocketWork};
use crate::gemini::GeminiEvent;

/// What to do with an inbound Gemini event before its own arm sees it.
///
/// The two ways a reply is unwanted, and they are not the same. A hold is a
/// standing condition: it silences output for as long as it lasts and nothing
/// about the event changes it. A discard is one turn's sentence, armed when a
/// hold cut a reply already in flight, and the turn's own end is what serves
/// it -- which is why `Deliver` is not the whole answer here and
/// `EndsTheDiscard` exists.
#[derive(Debug, PartialEq, Eq)]
pub(in crate::livekit) enum OutputDisposition {
    Deliver,
    Drop,
    EndsTheDiscard,
}

/// Whether an event is Gemini actually answering what it was prompted with.
///
/// Not every output event is. A `TurnComplete` or `Interrupted` can belong to
/// the generation before the prompt, and empty audio and blank text say
/// nothing; letting any of those settle a prompt leaves no reply
/// owed when the socket is replaced before the real answer.
pub(in crate::livekit) fn answers_prompt(event: &GeminiEvent) -> bool {
    match event {
        GeminiEvent::Audio { bytes, .. } => !bytes.is_empty(),
        GeminiEvent::ToolCall(_) => true,
        GeminiEvent::Text(text) | GeminiEvent::OutputTranscript(text) => !text.trim().is_empty(),
        GeminiEvent::UsageRecorded
        | GeminiEvent::InputTranscript(_)
        | GeminiEvent::CandidateVoiceActivity { .. }
        | GeminiEvent::TurnComplete
        | GeminiEvent::Interrupted
        | GeminiEvent::GoAway { .. } => false,
    }
}

/// Speech or text that reaches the candidate, which a hold or a discard
/// silences.
pub(in crate::livekit) fn is_output(event: &GeminiEvent) -> bool {
    matches!(
        event,
        GeminiEvent::Audio { .. } | GeminiEvent::OutputTranscript(_) | GeminiEvent::Text(_)
    )
}

/// Split out of `handle_gemini_event` because it is the whole of what that
/// function decides before dispatching, and none of it needs a room, a socket
/// or an await. It is also the rule a restart has to get right: the discard
/// belongs to the socket that armed it, and a replacement that inherits one
/// drops its own first turn, which is the cold-restart briefing.
pub(in crate::livekit) fn output_disposition(
    event: &GeminiEvent,
    discarding: bool,
    held: bool,
) -> OutputDisposition {
    let output = is_output(event);
    let ending = ends_turn(event);

    if discarding {
        if output {
            return OutputDisposition::Drop;
        }
        if ending {
            return OutputDisposition::EndsTheDiscard;
        }
    }

    // Checked after the discard, not before it: a turn ending while held still
    // has to serve the discard's sentence, and `TurnComplete` is not output so
    // it was never the thing a hold silences.
    if held && output {
        return OutputDisposition::Drop;
    }
    OutputDisposition::Deliver
}

/// A turn ending, whichever way it ends.
pub(in crate::livekit) fn ends_turn(event: &GeminiEvent) -> bool {
    matches!(event, GeminiEvent::TurnComplete | GeminiEvent::Interrupted)
}

pub(in crate::livekit) enum TurnBoundary {
    Current,
    Stale,
    InterruptedTail,
}

/// The old turn was already cut off by a hold. Its delayed ending must not
/// settle or cancel a prompt, candidate turn, or tool continuation sent since.
///
/// A `TurnComplete` the interruption guard swallows ends the discard too. With
/// both armed they wait on the same old turn, which sends one ending, and a
/// discard left behind it eats the next reply.
pub(in crate::livekit) fn turn_boundary(
    event: &GeminiEvent,
    socket: &mut SocketWork,
    held: bool,
) -> TurnBoundary {
    if socket.stale_turn_pending && ends_turn(event) {
        // An `Interrupted` is followed by the same turn's `TurnComplete`, which
        // ends the wait.
        socket.stale_turn_pending = matches!(event, GeminiEvent::Interrupted);
        return TurnBoundary::Stale;
    }
    if matches!(event, GeminiEvent::Interrupted) {
        socket.interrupted_turn_pending = true;
    } else if answers_prompt(event) && !socket.discarding_output && !held {
        // Not under a hold, even for a tool call, which is still delivered then
        // and counted as output below. The generation a hold cut off can keep
        // calling tools, and those calls are no sign that a newer turn began;
        // disarming on them handed its late ending the resume prompt. The cost
        // falls on a newer turn under a hold that only calls tools: its ending
        // is swallowed, and `generating` stays set until the stall check clears
        // it.
        socket.interrupted_turn_pending = false;
        socket.stale_turn_pending = false;
    }
    if matches!(event, GeminiEvent::TurnComplete)
        && std::mem::take(&mut socket.interrupted_turn_pending)
    {
        return TurnBoundary::InterruptedTail;
    }
    TurnBoundary::Current
}

impl RuntimeActivity {
    /// Decides what an inbound Gemini event is to the turn it arrived in,
    /// settles the activity's waits for it, and says whether it is delivered.
    pub(in crate::livekit) fn accept_event(
        &mut self,
        event: &GeminiEvent,
        held: bool,
        now: Instant,
    ) -> bool {
        // Even a hold that drops the queued greeting has consumed the probe's
        // observation. Subsequent work uses the normal output and recovery
        // clocks.
        if answers_prompt(event) {
            self.buffered_startup_output_at = None;
        }
        match turn_boundary(event, &mut self.socket, held) {
            TurnBoundary::Stale => return false,
            TurnBoundary::InterruptedTail => {
                self.end_discard(false);
                return false;
            }
            TurnBoundary::Current => {}
        }
        if matches!(event, GeminiEvent::Interrupted) {
            self.turn_completed_at = None;
        }
        match output_disposition(event, self.socket.discarding_output, held) {
            OutputDisposition::Drop => false,
            OutputDisposition::EndsTheDiscard => {
                self.end_discard(
                    matches!(event, GeminiEvent::Interrupted) && self.candidate_interrupting(now),
                );
                false
            }
            OutputDisposition::Deliver => {
                // Old tool calls still need responses, but their generation
                // cannot answer the resume prompt or refresh its progress
                // clock.
                if !self.socket.discarding_output {
                    if matches!(event, GeminiEvent::OutputTranscript(_))
                        && answers_prompt(event)
                        && self.is_trailing_transcript(now)
                    {
                        self.note_trailing_transcript();
                    } else if answers_prompt(event) {
                        self.note_output(now);
                    } else if ends_turn(event) {
                        let said_something = self.generating;
                        self.note_turn_boundary(now);
                        if said_something && matches!(event, GeminiEvent::TurnComplete) {
                            self.turn_completed_at = Some(now);
                        }
                    }
                }
                true
            }
        }
    }
}
