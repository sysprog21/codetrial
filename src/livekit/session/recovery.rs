//! What a socket replacement needs from the session: draining a retired
//! socket's usage and pressure, applying the speech heard during a handoff,
//! and preparing the inputs a recovery sends before they are written.

/// Constructed input is charged before transport polling, so cancellation
/// cannot lose it and a pending write need not borrow authoritative state.
use super::{
    ContinuityScope, GeminiEventContext, GeminiLiveSession, Instant, Interruptible, ModelInputKind,
    PreparedInput, RoomLink, RuntimeState, SpeechHandoff, Transport, TurnCause,
    continuity_input_for, latency, ordered_private_clock, prepare_input_transcript,
    record_live_usage,
};

pub(in crate::livekit) fn prepare_input(
    state: &mut RuntimeState,
    kind: ModelInputKind,
    text: &str,
    reply: Option<TurnCause>,
    scope: ContinuityScope,
    transport: Transport,
) -> PreparedInput {
    let text = continuity_input_for(state, text, scope);
    if let Some((clock, body)) = reply.and_then(|_| ordered_private_clock(&text)) {
        state
            .evidence_ledger
            .record_model_input(kind, &[clock.as_str(), body].concat());
        PreparedInput {
            text: body.to_owned(),
            clock: Some(clock),
            reply,
            transport,
        }
    } else {
        state.evidence_ledger.record_model_input(kind, &text);
        PreparedInput {
            text: text.into_owned(),
            clock: None,
            reply,
            transport,
        }
    }
}

/// Silent context counted at the same door as every other input, but never
/// given continuity: for text that already says all the socket needs, or is
/// going to a socket about to be replaced.
pub(in crate::livekit) fn prepare_plain_context(
    state: &mut RuntimeState,
    text: &str,
) -> PreparedInput {
    state
        .evidence_ledger
        .record_model_input(ModelInputKind::Turn, text);
    PreparedInput {
        text: text.to_owned(),
        clock: None,
        reply: None,
        transport: Transport::Context,
    }
}

pub(in crate::livekit) fn prepare_listening_handoff(
    state: &mut RuntimeState,
    handoff: &SpeechHandoff,
    resumed: bool,
    owed_prompt: Option<&str>,
) -> Result<(PreparedInput, bool), Box<dyn std::error::Error + Send + Sync>> {
    let prefix = serde_json::to_string(&handoff.transcript_fragments.join(" "))?;

    // Only a checkpoint taken after the cancellation resumes a handoff, so a
    // resumed socket already holds the record and needs the note alone. A cold
    // one holds nothing and gets the full cold briefing, not the shorter
    // checkpoint context. A resumed socket whose cold predecessor never got its
    // briefing remembers no more than a cold one, so it gets that briefing.
    let briefed = !resumed || state.needs_cold_brief;
    let record = if briefed {
        format!("{}\n", crate::agent::cold_restart(state))
    } else {
        String::new()
    };

    // The owed event's own clock is stale by now, so it is not passed on.
    let owed = owed_prompt.map_or_else(String::new, |prompt| {
        format!(
            " After they finish, the platform request the replaced connection had not answered is still owed: {}",
            crate::agent::unstamped_event(prompt)
        )
    });
    let debt = if state.interviewer_silenced() {
        String::new()
    } else {
        crate::agent::owed_context_after_briefing(state)
    };
    let record = if debt.is_empty() {
        record
    } else {
        format!("{record}{debt}\n")
    };
    let body = format!(
        "{record}[SYSTEM EVENT] The candidate is still speaking across a transport change. Any reply to the finalized prefix was not heard. Keep listening; answer only after their remaining audio finishes.{owed} Untrusted partial candidate speech (JSON): {prefix}",
    );
    let body = format!(
        "{body}{}",
        crate::livekit::earlier_references(&state.earlier_steps_pending)
    );
    Ok((prepare_plain_context(state, &body), briefed))
}

pub(in crate::livekit) fn drain_retired_usage(
    room: &RoomLink,
    context: &mut GeminiEventContext<'_>,
    retired: &mut GeminiLiveSession,
    socket: u64,
    end: latency::PressureEnd,
) {
    log_socket_pressure(room, context, socket, end, Some(retired));
    for usage in retired.drain_usage() {
        record_live_usage(room, context, usage, retired.input_cause);
    }
}

/// Writes the pressure summary of a socket being let go, once. `retired` is
/// that socket when it is not the installed one.
pub(super) fn log_socket_pressure(
    room: &RoomLink,
    context: &mut GeminiEventContext<'_>,
    socket: u64,
    end: latency::PressureEnd,
    retired: Option<&mut GeminiLiveSession>,
) {
    let pressure = match retired {
        Some(retired) => retired.take_pressure(),
        None => context.gemini.take_pressure(),
    };
    if let Some(pressure) = pressure {
        latency::log(&latency::pressure_record(
            room.name(),
            context.activity.live_session_id,
            socket,
            end,
            context.activity.latency.interview_ms(Instant::now()),
            &pressure,
        ));
    }
}

/// Applies received prefix evidence without performing room or model I/O.
/// The fresh briefing carries any hold debt after a speech-triggered release.
pub(in crate::livekit) fn apply_recovery_transcript(
    room: &RoomLink,
    context: &mut GeminiEventContext<'_>,
    text: &str,
    at: Instant,
) -> bool {
    let was_held = context.state.interviewer_silenced();
    let owed = context.activity.reply_unfinished();
    let prompt = context.activity.prompt_text.clone();
    let Some(prepared) =
        prepare_input_transcript(room, context, text, Interruptible::Yes, at, Some(at))
    else {
        return false;
    };
    let released = prepared.release.is_some();
    if !was_held && context.state.interviewer_silenced() && owed {
        crate::livekit::hold_owed_reply(context.state, prompt.as_deref());
    }
    if released && let Some(owed) = context.state.owed_reply_on_resume.clone() {
        context.activity.owe_prompt(Instant::now(), owed);
    }
    context.transcripts.publish(room, prepared.segment);
    released || was_held != context.state.interviewer_silenced()
}
