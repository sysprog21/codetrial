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
