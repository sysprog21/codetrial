//! Ending an interview: the farewell, and the wait for it to be heard.

// This module reads the room session's imports through `use super::*`.
#[cfg_attr(
    not(test),
    expect(
        clippy::wildcard_imports,
        reason = "the module reads its hub through super::*"
    )
)]
use super::*;

/// The goodbye has been said and has played. Settled output alone is not
/// enough: the wrap-up can go out behind an acknowledgement that stalled and
/// was released, and that turn's late ending settles the floor before the
/// goodbye has begun. The goodbye's own prompt stays owed across it, since it
/// went out behind a turn, so it is waited on too.
pub(in crate::livekit) fn goodbye_heard(activity: &RuntimeActivity, audio_playing: bool) -> bool {
    output_settled(activity.floor, audio_playing) && !activity.owes_prompt()
}

pub(in crate::livekit) async fn send_wrap_up_and_wait(
    room: &RoomLink,
    context: &mut GeminiEventContext<'_>,
    reason: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let farewell = with_timer(
        context.state,
        wrap_up(
            reason,
            context.state.behavioral_round_started
                && crate::agent::evidence_tool_declared(context.state.interview_mode),
        ),
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
            set_agent_state(room, context.agent_state, AGENT_STATE_LISTENING);
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
