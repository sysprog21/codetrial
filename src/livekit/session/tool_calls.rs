//! Executes and sends one interview tool batch.

use super::{
    GeminiEventContext, GeminiFunctionCall, HintCounters, Instant, RoomLink, TOOL_READ_BOARD,
    TOOL_RECORD_FRAMEWORK_EVIDENCE, ToolTiming, TurnCause, board, execute_timed_tool_batch,
    flush_framework_progress, known_tool_name, latency, log_clock, observations, play_tool_ack,
};

#[cfg(test)]
use crate::livekit::probe::acquisition::owner::{self, Stage};

/// Answers every call in the batch in one message, and republishes the
/// checklist once when any of them moved it.
pub(super) async fn on_tool_calls(
    room: &RoomLink,
    context: &mut GeminiEventContext<'_>,
    calls: Vec<GeminiFunctionCall>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if calls.is_empty() {
        return Ok(());
    }
    // Before every await below, any of which can cancel the batch.
    #[cfg(test)]
    crate::livekit::probe::acquisition::receive(
        &context.activity.latency,
        &calls,
        context.activity.socket.discarding_output || context.state.interviewer_silenced(),
    );
    // A filler that fails to queue is not worth the tool round it precedes.
    #[cfg(test)]
    owner::before(Stage::Acknowledgement, context).await;
    if let Err(error) = play_tool_ack(context, &calls).await {
        eprintln!("tool acknowledgement not played: {error}");
    }
    if context.state.example_drawing_enabled
        && !context.state.interview_mode.is_whiteboard()
        && calls.iter().any(|call| call.name == TOOL_READ_BOARD)
    {
        #[cfg(test)]
        owner::before(Stage::BoardSettlement, context).await;
        context.board.settle_for_read(context.state).await;
    }
    let timed = !context.state.interviewer_silenced() && !context.activity.socket.discarding_output;
    let reviewed_revision = context.activity.substantive_revision_at_last_review;
    let tool_started = Instant::now();
    let hints_before = HintCounters::capture(context.state);
    #[cfg(test)]
    let mut ticket = None;
    let answers = execute_timed_tool_batch(
        context.state,
        context.activity,
        calls,
        reviewed_revision,
        timed,
        #[cfg(test)]
        &mut ticket,
    );
    let tool_execute_ms = crate::agent::saturating_ms(tool_started.elapsed());
    let editor_review_revision = answers
        .iter()
        .any(|(_, answer)| answer.get("editorReview").is_some())
        .then_some(context.state.evidence_ledger.code.substantive_revision);

    // What `read_board` could not put in its own response. Once per batch, so a
    // batch that asked twice puts the board up once, and before the responses
    // rather than after: Gemini starts its reply as soon as it has them, so a
    // board sent after would arrive under an answer already being spoken about
    // the board before it, while the tool has said this one is in front of it.
    //
    // Not `?`: an image the socket would not take is worth a line and a turn
    // that answers from the board it already had, not an interview ended on the
    // write.
    let board_started = Instant::now();
    if std::mem::take(&mut context.state.board_resend_requested)
        && let Err(error) = {
            #[cfg(test)]
            owner::before(Stage::BoardResend, context).await;
            board::resend(context.board, context.gemini).await
        }
    {
        eprintln!("board resend failed ({error}); waiting for the close to be reported");
    }
    let board_resend_ms = crate::agent::saturating_ms(board_started.elapsed());

    // Not `?`. A response that did not go out is owed nothing back, so the flag
    // stays down, and the socket it failed on is replaced when its close is
    // reported; the checklist below still reflects the calls.
    #[cfg(test)]
    owner::before(Stage::ToolSend, context).await;
    let send_started = Instant::now();
    let sent = context.gemini.send_tool_responses(&answers).await;
    let tool_send_ms = crate::agent::saturating_ms(send_started.elapsed());
    #[cfg(test)]
    if let Some(ticket) = ticket {
        ticket.complete(sent.is_ok());
    }
    if timed {
        context
            .activity
            .turn_timing
            .note_tool_response(send_started, Instant::now(), sent.is_ok());
    }

    // Gemini resumes the moment the responses land, so serializing them again
    // for the lines below waits until after the send.
    if !context.activity.socket.discarding_output && !context.state.interviewer_silenced() {
        let timing = observations::tool_timing(
            context.activity,
            context.state,
            &answers,
            hints_before,
            ToolTiming {
                batch_ms: crate::agent::saturating_ms(tool_started.elapsed()),
                execute_ms: tool_execute_ms,
                board_ms: board_resend_ms,
                send_ms: tool_send_ms,
                sent: sent.is_ok(),
            },
        );
        latency::log(&timing);
    }
    for (call, response) in &answers {
        let name = known_tool_name(&call.name);
        eprintln!(
            "codetrial live_tool_response room={} tool={name} bytes={} accepted={}",
            room.name(),
            response.to_string().len(),
            response.get("error").is_none(),
        );

        // The phase as the checklist spells it, and only when it is one: the
        // argument is model text, and the rest of the call stays out of the log
        // for the reason the checklist carries no summary.
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
    }

    if let Some(revision) = editor_review_revision {
        eprintln!(
            "codetrial live_editor_review room={} revision={revision} sent={}",
            room.name(),
            sent.is_ok(),
        );
    }
    context.activity.settle_tool_editor_review(
        editor_review_revision,
        sent.is_ok(),
        Instant::now(),
    );

    // A reminder of open earlier steps in a response that never went out has
    // not been read, so it may be named again.
    context.state.settle_earlier_steps(sent.is_ok());
    match sent {
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
    flush_framework_progress(room, context.state).await;
    Ok(())
}
