//! Where a Gemini event falls against the turn it belongs to: the current
//! turn, the late ending of one a hold already cut off, or the tail of an
//! interrupted one. The room loop and the startup speech an early End
//! preserves both judge it the same way.

// This module reads the room session's imports through `use super::*`.
#[cfg_attr(
    not(test),
    expect(
        clippy::wildcard_imports,
        reason = "the module reads its hub through super::*"
    )
)]
use super::*;
use crate::livekit::turn::SocketWork;

pub(super) enum TurnBoundary {
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
pub(super) fn turn_boundary(
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

/// Only evidence is owed before freezing an End that overtook startup replay.
/// Do not play queued output, execute tools, request a reply or wait for more
/// transcription. Closing at the original boundaries keeps separate utterances
/// separate; removing only their input events prevents farewell replay doubling
/// them while leaving the ordinary output and usage path intact.
pub(in crate::livekit) fn preserve_startup_speech(
    room: &RoomLink,
    context: &mut GeminiEventContext<'_>,
) {
    let identity = context.media.identity.clone();
    let mut socket = context.activity.socket.clone();
    let held = context.state.interviewer_silenced();
    for event in context.gemini.take_buffered_startup_speech() {
        match turn_boundary(&event, &mut socket, held) {
            TurnBoundary::Stale => continue,
            TurnBoundary::InterruptedTail => {
                socket.discarding_output = false;
                continue;
            }
            TurnBoundary::Current => {}
        }
        if let GeminiEvent::InputTranscript(text) = &event {
            if let Some(identity) = identity.as_deref()
                && transcript_text(text).is_some()
            {
                let segment = record_candidate_transcript(context, text, identity);
                context.transcripts.publish(room, segment);
            }
        } else if ends_turn(&event) {
            if output_disposition(&event, socket.discarding_output, held)
                == OutputDisposition::Deliver
            {
                close_turns(room, context);
            } else {
                socket.discarding_output = false;
            }
        }
    }
}
