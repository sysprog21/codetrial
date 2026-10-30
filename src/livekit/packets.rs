//! What the browser sends: one decoded data packet at a time, from the hold
//! the Thinking button takes to the ending an `end_interview` control writes
//! the report from. The other two sources the loop drains are elsewhere,
//! `handle_gemini_event` in `session` and `handle_media_event` in `media`.
//!
//! Like `session`, this borrows `GeminiEventContext` for the length of one
//! packet and hands it straight back, and names the loop's own items through
//! `super` for that one handover.

use super::*;

/// The candidate chose Thinking at `at`. Ending the audio stream makes Gemini
/// transcribe what came before the click now, inside the window
/// `ignore_input_before_hold` opens, rather than a silence window later; in a
/// measured session the transcript followed the stream end by about 0.3s. The
/// reply Gemini starts for it is dropped by the hold.
pub(super) async fn begin_button_hold(
    gemini: &mut GeminiLiveSession,
    candidate_audio: &mut Vec<u8>,
    activity: &mut RuntimeActivity,
    at: Instant,
    grace: Duration,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    activity.clear_candidate_voice_activity();
    activity.ignore_input_before_hold(at, grace);
    activity.latency.close(latency::Boundary::Held, at);
    flush_audio(gemini, candidate_audio).await?;
    gemini.end_audio_turn().await
}

/// Told to the model when a Thinking click cuts off a reply it was giving.
pub(super) const THINKING_CUT_OFF: &str = "[SYSTEM EVENT] The candidate is thinking and has kept the floor. Any reply interrupted by this hold was not heard in full. Stay silent until they speak again or yield the turn.";

/// What a control packet's thinking outcome leaves for the room to do.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct HoldEffects {
    /// Jim was cut off: his turn has to be closed and his state shown as
    /// listening.
    pub(super) cut_off: bool,
    /// Finalizing the candidate's turn failed and its reply is marked owed,
    /// so nothing more is sent for this packet.
    pub(super) abandoned: bool,
}

/// The room-free half of a control packet's thinking outcome: the socket
/// writes and the activity bookkeeping, split from `handle_data_packet` so a
/// fake socket can drive it. `reply` is the prompt the packet asks for, and
/// comes back `None` when this has already delivered it as context.
///
/// A new hold ends the audio stream and cuts off a reply in progress. Ending
/// a hold with something to say, or yielding, finalizes the candidate's turn:
/// buffered audio goes, then the stream ends so Gemini answers without waiting
/// out its silence window. A candidate turn still open takes the prompt as
/// context instead, since a realtime prompt would answer before the
/// candidate's own words.
pub(super) async fn settle_hold(
    context: &mut GeminiEventContext<'_>,
    result: &crate::agent::DataEventResult,
    reply: &mut Option<String>,
    at: Instant,
    grace: Duration,
) -> HoldEffects {
    let mut effects = HoldEffects::default();
    if result.thinking_changed == Some(true) {
        if let Err(error) = begin_button_hold(
            context.gemini,
            &mut context.media.audio_bytes,
            context.activity,
            at,
            grace,
        )
        .await
        {
            eprintln!(
                "Gemini audio stream end failed ({error}); waiting for the close to be reported"
            );
        }
        let had_floor = context.activity.floor != Floor::Listening;
        session::cut_off_for_hold(context.state, context.activity, context.output_audio);
        if had_floor {
            // Owed until a prompt that carries it is sent: the note below can
            // fail to send, and a resumed socket keeps the partial reply.
            context.state.thinking_unheard_reply = true;
            effects.cut_off = true;
            if let Err(error) = send_model_context(
                context.gemini,
                context.state,
                ModelInputKind::Turn,
                THINKING_CUT_OFF,
                None,
            )
            .await
            {
                eprintln!(
                    "Gemini thinking context failed ({error}); waiting for the close to be reported"
                );
            }
        }
    }
    if result.thinking_changed == Some(false) {
        context.activity.resume_reply_wait(at);
    }
    let release = result.thinking_changed == Some(false) && reply.is_some();
    if !(result.yield_turn || release) {
        return effects;
    }
    context.activity.clear_candidate_voice_activity();
    if result.yield_turn {
        context.activity.latency.yielded(at);
    }
    let finalize_prompt = reply.clone();
    let finalize = async {
        flush_audio(context.gemini, &mut context.media.audio_bytes).await?;
        if context.turns_candidate_open()
            && let Some(prompt) = reply.as_deref()
        {
            context.activity.latency.recovery_prompt(at);
            send_model_context(
                context.gemini,
                context.state,
                ModelInputKind::Turn,
                prompt,
                None,
            )
            .await?;
            context.activity.defer_thinking_reply(prompt);
            if !context.activity.socket.discarding_output {
                context.activity.arm_thinking_reply_fallback(at, prompt);
            }
            *reply = None;
            context.state.clear_thinking_debt();
        }
        context.gemini.end_audio_turn().await
    }
    .await;
    if let Err(error) = finalize {
        eprintln!(
            "Gemini turn finalization failed ({error}); waiting for the close to be reported"
        );
        context
            .activity
            .mark_recovery_prompted(at, finalize_prompt.as_deref());
        effects.abandoned = true;
    }
    effects
}

/// Ends the interview by handing the loop the packet the browser would send.
///
/// Three routes reach the same ending now -- the candidate's own button, the
/// server-side deadline, and the interviewer's `end_interview` tool -- and the
/// last two arrive at the first one's path rather than at a copy of it, because
/// that is where the wrap-up, the report and the teardown are, and where they
/// are already tested. The result is always `Break` and the report has been
/// published by the time this returns, so there is nothing for a caller to do
/// but stop -- which holds because both callers check `ended` first. Called on
/// an interview that has already ended it would return having published
/// nothing, and the caller would stop just the same.
pub(super) async fn end_through_control(
    room: &RoomLink,
    context: &mut GeminiEventContext<'_>,
    interview: InterviewContext<'_>,
    reason: &str,
    review: &mut SideCall,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // A review that came back between two watch ticks is a pause already paid
    // for, and the report is about to be written. Collected here rather than
    // left to the tick that will not run, because the alternative is the drop
    // below aborting a result that had already arrived.
    if let Some(finished) = review.finished()
        && let Ok(notes) = finished.await
    {
        record_interim_notes(context.state, &notes);
    }
    let _ = handle_data_packet(
        room,
        context,
        interview,
        TOPIC_CONTROL,
        &serde_json::json!({ "type": "end_interview", "reason": reason }),
        false,
    )
    .await?;
    Ok(())
}

/// Applies one decoded data packet. `Break` means the interview is over and the
/// report has been published.
pub(super) async fn handle_data_packet(
    room: &RoomLink,
    context: &mut GeminiEventContext<'_>,
    interview: InterviewContext<'_>,
    topic: &str,
    payload: &serde_json::Value,
    received: bool,
) -> Result<ControlFlow<()>, Box<dyn std::error::Error + Send + Sync>> {
    if settles_phase_judge(context.state, topic, payload) {
        settle_phase_judge(context, interview).await;
    }
    let judge_running = context.activity.phase_judge.is_running();
    if defers_round_transition(context.state, judge_running, topic, payload)
        && (judge_running || context.activity.reserve_phase_judge(Instant::now()))
    {
        if !judge_running {
            let judgment = spawn_phase_judge(context.state, interview);
            context.activity.phase_judge.start(judgment);
        }
        // A repeat of the same request keeps the first one's hold.
        context
            .activity
            .pending_round_transition
            .get_or_insert_with(|| ParkedPacket {
                payload: payload.clone(),
                received,
                deadline: Instant::now() + PHASE_JUDGE_SETTLE,
            });
        return Ok(ControlFlow::Continue(()));
    }
    apply_data_packet(room, context, interview, topic, payload, received).await
}

pub(super) fn reduce_data_packet(
    state: &mut RuntimeState,
    activity: &mut RuntimeActivity,
    topic: &str,
    payload: &serde_json::Value,
    received: bool,
    receipt_timestamp_ms: u64,
) -> crate::agent::DataEventResult {
    let code_revision_before = state.evidence_ledger.code.revision;
    let test_runs_before = state.test_runs;
    let result = if received {
        apply_data_event_at(
            state,
            topic,
            payload,
            activity.last_test_reaction.elapsed().as_secs_f64(),
            receipt_timestamp_ms,
        )
    } else {
        crate::agent::apply_server_event_at(
            state,
            topic,
            payload,
            activity.last_test_reaction.elapsed().as_secs_f64(),
            receipt_timestamp_ms,
        )
    };
    activity.observe_candidate_work(state, code_revision_before, test_runs_before);
    result
}

/// Applies a packet after any judgment wait has been settled. Released round
/// transitions enter here so new speech or a slow judge cannot renew the hold.
pub(super) async fn apply_data_packet(
    room: &RoomLink,
    context: &mut GeminiEventContext<'_>,
    interview: InterviewContext<'_>,
    topic: &str,
    payload: &serde_json::Value,
    received: bool,
) -> Result<ControlFlow<()>, Box<dyn std::error::Error + Send + Sync>> {
    // One reading per packet, shared by every entry the packet produces.
    let receipt_timestamp_ms = crate::current_epoch_millis();
    let packet_at = Instant::now();
    let was_requested = context.state.thinking_hold.is_requested();
    let mut result = reduce_data_packet(
        context.state,
        context.activity,
        topic,
        payload,
        received,
        receipt_timestamp_ms,
    );
    if result.update_last_code_change {
        context.activity.last_code_change = Instant::now();
    }
    if result.update_last_test_reaction {
        context.activity.last_test_reaction = Instant::now();
    }
    if result.update_last_interjection {
        context.activity.last_interjection = Instant::now();
    }
    context
        .activity
        .settle_thinking_request(was_requested, context.state, &result);

    // Before the reply goes to the model, so a tick the packet earned reaches
    // the page without waiting on that write.
    session::flush_framework_progress(room, context.state).await;
    let mut reply = result.generate_reply.take();
    if let Some(summary) = context
        .activity
        .turn_timing
        .finish_thinking_change(Instant::now(), result.thinking_changed)
    {
        session::log_turn_timing(
            room.name(),
            context.activity,
            &log_clock(context.state),
            summary,
            &mut std::io::stderr().lock(),
        );
    }
    let grace = thinking_transcript_grace(interview.boot.silence_ms);
    if result.thinking_changed == Some(true) {
        session::observe_floor(
            room.name(),
            context.activity,
            context.state,
            floor::Event::Reset,
        );
    }
    let effects = settle_hold(context, &result, &mut reply, packet_at, grace).await;
    if effects.cut_off {
        close_turns(room, context);
        set_agent_state(room, context.agent_state, AGENT_STATE_LISTENING);
    }

    // The reply is already marked owed. Everything else the packet changed
    // still has to reach the page: a round it started cannot be started again.
    if effects.abandoned {
        reply = None;
    }

    // Whether what this packet asked the model to read reached it, which is
    // what settles a reminder of open earlier steps written into it.
    let mut delivered = false;
    if let Some(held) = result.held_context.take() {
        let held = crate::agent::with_timer(context.state, held);
        match send_model_context(
            context.gemini,
            context.state,
            ModelInputKind::Turn,
            &held,
            None,
        )
        .await
        {
            Ok(()) => delivered = true,
            Err(error) => eprintln!(
                "Gemini held context failed ({error}); waiting for the close to be reported"
            ),
        }
    }
    if let Some(status) = result.round_changed {
        publish_control(
            room,
            &serde_json::json!({
                "type": "round_state", "round": "behavioral", "status": status
            }),
            "the round state",
        )
        .await;
    }

    // Every result the browser reports, reacted to or not: one inside the
    // reaction cooldown updates the record without Jim saying anything, and a
    // later prompt that describes it is otherwise unexplained.
    if let Some(judged) = test_run_line(topic, context.state, &result, reply.is_some()) {
        eprintln!(
            "tests: at={} {judged} room={}",
            log_clock(context.state),
            interview.boot.room_name
        );
    }
    if let Some(prompt) = reply {
        // Not `?`: a failed write here ended the interview with no report, and
        // the socket it failed on is replaced when the close is reported.
        let sent = send_model_text(
            context.gemini,
            context.state,
            ModelInputKind::Turn,
            TurnCause::Turn,
            &prompt,
        )
        .await;
        match record_event_prompt(context.activity, &prompt, sent, Instant::now()) {
            Ok(()) => {
                delivered = true;
                if result.carries_thinking_debt {
                    context.state.clear_thinking_debt();
                }

                // Which data event made Jim speak, against the progress it was
                // sent with, so a transcript that repeats a step can be matched
                // to the prompt that asked for it. After the send, so it
                // records a prompt that reached the socket.
                eprintln!(
                    "{}",
                    prompt_line(
                        context.state,
                        context.activity,
                        &format!("topic={topic}"),
                        interview.boot.room_name,
                    )
                );
            }
            Err(error) => {
                // The debt the prompt carried is still in the state, unpaid,
                // for the next socket.
                if owes_failed_reply(&result) {
                    context
                        .activity
                        .mark_prompted(Instant::now(), Some(&prompt), false);
                }
                eprintln!(
                    "Gemini reply request failed ({error}); waiting for the close to be reported"
                );
            }
        }
    }
    context.state.settle_earlier_steps(delivered);
    let Some(reason) = result.finish_interview else {
        return Ok(ControlFlow::Continue(()));
    };
    if let Some(slot) = interview.slot {
        slot.assessment_finished();
    }

    session::flush_thinking_notice(room, context.state).await;

    // The assessment ends here, before the goodbye: the reducer has closed the
    // unasked steps of a round that never opened, and the turns still open are
    // the candidate's last words. The report is written from this point while
    // the farewell plays, so the candidate waits for the longer of the two
    // rather than for both. The farewell stays out of what is assessed: all it
    // can add is the skips a started behavioral round leaves to it, and the
    // report scores those steps as unassessed with or without them.
    //
    // Nothing between here and `publish_with_recovery` propagates an error. The
    // turns go out as a LiveKit text stream and the report as a data packet, so
    // the one failing says nothing about the other, and a final segment the
    // panel never saw is cosmetic where a missing report is not.
    let assessment = finish_assessment(room, context, interview).await;
    let report_boards = assessment.report_boards();
    let mut recovery_events = interview.events.lock().await;
    let api_key = &**interview.keys;
    let farewell = async {
        // The goodbye is the only part of the ending that needs the Live
        // socket. The report is written over HTTP from what this process
        // already holds, so a socket that died under the goodbye costs the
        // goodbye and nothing else; propagating here dropped the report call
        // and left the candidate waiting on a report nobody was writing.
        if should_send_wrap_up(&reason)
            && let Err(error) = send_wrap_up_and_wait(room, context, &reason).await
        {
            eprintln!("wrap-up failed ({error}); writing the report without it");
        }

        // The goodbye is the last turn there will be. Closing here keeps the
        // panel from leaving it rendered as still-in-progress for the rest of
        // the page's life, and costs one publish.
        close_turns(room, context);
        context.transcripts.flush(TRANSCRIPT_FLUSH_LIMIT).await;
    };
    let generation = async {
        tokio::join!(
            generate_report_bounded(
                interview.boot,
                &assessment.prompt,
                &report_boards,
                (assessment.rounds(), assessment.drawing_received()),
                api_key,
                crate::gemini::GENERATION_SEED,
                &assessment.refused,
            ),
            farewell
        )
    };

    // The candidate can give up on the report while it is being written. This
    // handler is the only thing reading the room until it returns, so the wait
    // is raced against a `skip_report` packet. `biased` so a skip that arrives
    // with the report loses nothing: the report is then discarded rather than
    // published. Dropping `generation` stops the wait on the model; a request
    // already on the wire still finishes server-side, and its answer is never
    // read.
    let outcome = tokio::select! {
        biased;
        () = report::wait_for_skip(&mut recovery_events, interview.candidate_identity) => None,
        done = generation => Some(done),
    };
    context.media.audio = None;
    context.media.video = None;
    context.media.audio_bytes.clear();
    let Some((generated, ())) = outcome else {
        eprintln!(
            "candidate skipped the report; discarding it room={}",
            interview.boot.room_name
        );
        // A stalled ACK must not delay Gemini shutdown indefinitely.
        let _ = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            publish_control(
                room,
                &report::skip_report_ack(),
                "skip report acknowledgement",
            ),
        )
        .await;
        if let Err(error) = context.gemini.shutdown().await {
            eprintln!("Gemini close failed ({error}); leaving anyway");
        }
        leave_room(room).await;
        return Ok(ControlFlow::Break(()));
    };
    let close_live = async {
        if let Err(error) = context.gemini.shutdown().await {
            eprintln!("Gemini close failed ({error}); leaving anyway");
        }
    };
    report::publish_with_recovery(
        report::ReportRecovery {
            boot: interview.boot,
            assessment,
            reason: &reason,
            keys: api_key,
            candidate: interview.candidate_identity,
        },
        generated,
        &mut recovery_events,
        room.connected(),
        close_live,
    )
    .await?;

    // Every report went out through a receipt wait, so leaving now drops
    // nothing the candidate has not acknowledged or stopped listening for.
    leave_room(room).await;
    Ok(ControlFlow::Break(()))
}

pub(super) async fn finish_assessment(
    room: &RoomLink,
    context: &mut GeminiEventContext<'_>,
    interview: InterviewContext<'_>,
) -> report::FrozenAssessment {
    session::preserve_startup_speech(room, context);
    close_turns(room, context);
    context.transcripts.flush(TRANSCRIPT_FLUSH_LIMIT).await;

    // The board the browser sent just before ending can still be on its way;
    // see `BOARD_FINAL_WAIT`. Copied out rather than borrowed: the farewell
    // holds the context, board and all, while the report call runs beside it,
    // and a regeneration grades the same boards. Phase checkpoints preserve
    // work the candidate cleared before the final board.
    context.board.settle_for_report(context.state).await;
    freeze_assessment(
        interview.boot,
        context.state,
        interview.elapsed_min(),
        context.board.report_boards(),
    )
}

/// The `tests:` log line for a packet, which only a test-results packet has:
/// every one is logged, reacted to or not, since one inside the reaction
/// cooldown updates the record without the interviewer saying anything.
pub(super) fn test_run_line<'a>(
    topic: &str,
    state: &'a RuntimeState,
    result: &crate::agent::DataEventResult,
    reacted: bool,
) -> Option<TestRunLine<'a>> {
    (topic == crate::runtime::TOPIC_TEST_RESULTS).then(|| TestRunLine {
        run: state.last_test_run.as_ref(),
        note: result.test_run,
        credited_run_is_current: crate::agent::tested_code_is_current(state),
        reacted,
        ended: state.ended,
    })
}

/// Whether a reply this packet asked for, and that failed to send, is still
/// owed. Only one that carried a thinking hold's debt is: the debt stays in
/// the state for the next socket.
pub(super) fn owes_failed_reply(result: &crate::agent::DataEventResult) -> bool {
    result.carries_thinking_debt
}
