//! Replacing the Gemini socket: why one is replaced, what the new one is told
//! about the interview so far, and what the candidate is shown while it
//! happens.

// This module reads the hub's imports and types through `use
// crate::livekit::*`.
#[cfg_attr(
    not(test),
    expect(
        clippy::wildcard_imports,
        reason = "the module reads its hub through crate::livekit::*"
    )
)]
use crate::livekit::*;

/// The bare notice; `reconnect_wait` adds why a replacement is waiting.
pub(in crate::livekit) fn interviewer_state(reconnecting: bool) -> serde_json::Value {
    serde_json::json!({ "type": "interviewer_state", "reconnecting": reconnecting })
}

/// The notice a replacement shows while it waits on `reason`. `None` is the
/// attempt itself, whose length no one knows.
pub(in crate::livekit) fn reconnect_wait(
    reason: &str,
    wait: Option<Duration>,
) -> serde_json::Value {
    let mut notice = interviewer_state(true);
    notice["reason"] = reason.into();
    if let Some(wait) = wait {
        notice["waitSeconds"] = report::whole_seconds(wait).into();
    }
    notice
}

/// The Live socket cannot be kept, so the interview ends the way the deadline
/// ends it, with no goodbye to ask for. Already-ended sessions are not ended
/// twice; the room is left all the same.
pub(in crate::livekit) async fn end_without_interviewer(
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
        room.leave().await;
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
pub(in crate::livekit) fn hand_over(
    state: &mut RuntimeState,
    activity: &mut RuntimeActivity,
    probes: &mut TurnProbes,
    output_audio: &mut OutputAudio,
) -> (bool, String, Option<String>) {
    let (owed, debt, owed_prompt) = activity.take_handover_debt();
    cut_off_turn(state, activity, output_audio);
    // Late fragments belong to the retired socket, never its replacement.
    state.interviewer_delivery.replace();
    clear_abandoned_socket_work(state, activity, probes);
    activity.forget_delivered_context();
    (owed, debt, owed_prompt)
}

/// How a socket was replaced, as far as the briefing sent to it cares.
#[derive(Clone, Copy)]
pub(in crate::livekit) enum Replacement {
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
    pub(in crate::livekit) fn owed(self) -> bool {
        match self {
            Self::Cold { owed } | Self::Resumed { owed } => owed,
        }
    }
}

/// What made the socket be replaced.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::livekit) enum Trigger {
    /// The reply watchdog gave up on a socket that was not answering.
    Watchdog,
    /// The server hung up before an advisory was acted on.
    Close,
    /// A `GoAway` advisory was acted on, held or not.
    GoAway,
}

impl Trigger {
    pub(in crate::livekit) fn as_str(self) -> &'static str {
        match self {
            Self::Watchdog => "watchdog",
            Self::Close => "close",
            Self::GoAway => "goaway",
        }
    }
}

/// Why a replacement did not resume, or `None` when it did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::livekit) enum Fallback {
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
pub(in crate::livekit) enum Handover {
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
    pub(in crate::livekit) fn as_str(self) -> &'static str {
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
#[expect(
    clippy::fn_params_excessive_bools,
    reason = "five independent observations of one resume attempt, tested as a truth table"
)]
pub(in crate::livekit) fn fallback_reason(
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

pub(in crate::livekit) fn handover_kind(fallback: Fallback) -> Handover {
    match fallback {
        Fallback::None => Handover::Resumed,
        Fallback::CheckpointBeforeHandoff => Handover::ColdHandoff,
        _ => Handover::Degraded,
    }
}

/// The fields of the `replacement:` line the soak reads to judge a handover, in
/// the spelling it parses. Kept as `key=value` tokens so a new field never
/// moves the match.
pub(in crate::livekit) fn replacement_fields(
    trigger: Trigger,
    handover: Handover,
    fallback: Fallback,
) -> String {
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
pub(in crate::livekit) fn replacement_notice(handover: Handover) -> &'static str {
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
pub(in crate::livekit) fn replacement_notice_line(handover: Handover, room: &str) -> String {
    format!("{} room={room}", replacement_notice(handover))
}

/// Tells a replacement socket what it cannot know on its own, and reports
/// whether the briefing asked for a reply.
///
/// A resumed checkpoint is the last turn boundary the server marked resumable,
/// so it may predate the latest test run or spoken answer; a cold one knows
/// nothing. Both get the local record. Kept apart from `replace_gemini_session`
/// so a test drives the same branch production does.
#[cfg(test)]
pub(in crate::livekit) async fn brief_replacement(
    gemini: &mut GeminiLiveSession,
    state: &mut RuntimeState,
    activity: &mut RuntimeActivity,
    probes: &mut TurnProbes,
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
            let now = Instant::now();
            session::mark_recovery_prompted(activity, probes, now, owed_prompt);
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
#[cfg(test)]
pub(in crate::livekit) fn keep_recovery_debt(
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

pub(in crate::livekit) struct RecoveryBrief {
    pub(in crate::livekit) input: session::PreparedInput,
    pub(in crate::livekit) reply: bool,
    pub(in crate::livekit) clear_debt: bool,
    pub(in crate::livekit) clear_cold: bool,
}

pub(in crate::livekit) fn prepare_recovery_brief(
    state: &mut RuntimeState,
    replacement: Replacement,
    owed_prompt: Option<&str>,
) -> RecoveryBrief {
    let owed = replacement.owed();
    let held = state.interviewer_silenced();
    if owed && held {
        state.hold_owed_reply(owed_prompt);
    }
    let (text, reply, transport, scope, clear_debt, clear_cold) =
        if matches!(replacement, Replacement::Resumed { .. }) && !state.needs_cold_brief {
            let reply = owed && !held;
            let mut text = crate::agent::resumed_context(state, reply, owed_prompt);
            if reply {
                text = crate::agent::with_thinking_debt(state, &text);
            }
            (
                text,
                reply,
                session::Transport::Context,
                ContinuityScope::Resumed,
                reply,
                false,
            )
        } else if held {
            (
                crate::agent::cold_restart(state),
                false,
                session::Transport::Context,
                ContinuityScope::ColdOrCheckpoint,
                false,
                true,
            )
        } else {
            (
                crate::agent::with_owed_reply(
                    crate::agent::cold_restart(state),
                    owed.then_some(owed_prompt),
                ),
                true,
                session::Transport::Realtime,
                ContinuityScope::ColdOrCheckpoint,
                true,
                false,
            )
        };
    let text = format!("{text}{}", earlier_references(&state.earlier_steps_pending));
    let text = crate::agent::with_timer(state, text);
    let input = session::prepare_input(
        state,
        ModelInputKind::Turn,
        &text,
        reply.then_some(TurnCause::Recovery),
        scope,
        transport,
    );
    RecoveryBrief {
        input,
        reply,
        clear_debt,
        clear_cold,
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
#[cfg(test)]
pub(in crate::livekit) async fn send_recovery_brief(
    gemini: &mut GeminiLiveSession,
    state: &mut RuntimeState,
    replacement: Replacement,
    owed_prompt: Option<&str>,
) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
    let code = state.code.clone();
    let RecoveryBrief {
        input,
        reply,
        clear_debt,
        clear_cold,
    } = prepare_recovery_brief(state, replacement, owed_prompt);
    input.send(gemini).await?;
    state.code_shown = code;
    if clear_debt {
        state.clear_thinking_debt();
    }
    if clear_cold {
        state.needs_cold_brief = false;
    }
    Ok(reply)
}

/// Drops work that could only have been completed by the replaced socket.
///
/// A close request follows its tool acknowledgement: without that generation,
/// the loop must not turn a request from the old socket into a closing from the
/// new one. The other two flags are the same kind of debt. A closed socket
/// cannot send the event that clears them, and carrying either into its
/// replacement makes the new interviewer discard output or wait on work it did
/// not create.
pub(in crate::livekit) fn clear_abandoned_socket_work(
    state: &mut RuntimeState,
    activity: &mut RuntimeActivity,
    probes: &mut TurnProbes,
) {
    probes.turn_timing.reset();
    activity.abandon_socket_work(state);
}
