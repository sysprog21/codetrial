//! Work the room loop starts and does not wait on: the interim review of the
//! stretch nobody has assessed yet, and the phase judge that decides whether a
//! round may move on. Each runs as a detached call to Gemini and is collected
//! on a later watch tick.

// This module reads the hub's imports and types through `use super::*`.
#[cfg_attr(
    not(test),
    expect(
        clippy::wildcard_imports,
        reason = "the module reads its hub through super::*"
    )
)]
use super::*;

/// Reads the stretch of interview nobody has assessed yet, off to one side.
///
/// The room loop never waits on this. That is the whole point: the evaluation
/// that used to happen after the candidate stopped talking happens here
/// instead, in a pause, while the loop carries on handling their next word. It
/// is collected on a later watch tick, once the handle reports itself finished,
/// and nothing fails if it never does.
pub(super) fn spawn_interim_review(
    state: &mut RuntimeState,
    interview: InterviewContext<'_>,
) -> tokio::task::JoinHandle<String> {
    let prompt = take_interim_review_window(state, interview.boot);
    spawn_side_call(SideCallKind::SideCall, prompt, interview)
}

/// Which side call `spawn_side_call` makes. Its label opens the log line a
/// failed call leaves, which `scripts/analyze-gemini-usage.py` counts.
#[derive(Clone, Copy)]
enum SideCallKind {
    SideCall,
    PhaseJudge,
}

/// Starts one side call on the report keys, answered with nothing when it
/// fails: logged, and never the candidate's problem. An empty interim note
/// records nothing, and an empty judgment ticks nothing; the next pause or
/// turn asks again.
fn spawn_side_call(
    kind: SideCallKind,
    prompt: String,
    interview: InterviewContext<'_>,
) -> tokio::task::JoinHandle<String> {
    let keys = Arc::clone(interview.keys);
    let model = interview.boot.report_model.to_string();
    let room_name = interview.boot.room_name.to_string();
    tokio::spawn(async move {
        let (label, result) = match kind {
            SideCallKind::SideCall => (
                "interim review",
                generate_interim_review_with_keys(&keys, &model, &prompt, &room_name).await,
            ),
            SideCallKind::PhaseJudge => (
                "phase judge",
                generate_phase_judgment_with_keys(&keys, &model, &prompt, &room_name).await,
            ),
        };
        result.unwrap_or_else(|error| {
            eprintln!(
                "{label} skipped room={room_name}: {}",
                keys.redact(&error.to_string())
            );
            String::new()
        })
    })
}

/// How long the end of the interview waits for the phase judge in all, and
/// how long a round transition is held for it. Less than the call's own
/// deadline: the candidate is waiting on the report or on the next question.
pub(super) const PHASE_JUDGE_SETTLE: Duration = Duration::from_secs(5);

/// Reads the latest stretch for REACTO steps the interviewer has not recorded,
/// off to one side like the interim review. Collected on a later watch tick.
pub(super) fn spawn_phase_judge(
    state: &mut RuntimeState,
    interview: InterviewContext<'_>,
) -> tokio::task::JoinHandle<String> {
    let prompt = crate::agent::take_phase_judge_window(state, interview.boot.problem);
    spawn_side_call(SideCallKind::PhaseJudge, prompt, interview)
}

/// Applies a finished judgment, logs what it recorded the way the
/// interviewer's own evidence calls are logged, and tells the interviewer.
async fn collect_phase_judgment(context: &mut GeminiEventContext<'_>, text: &str, room_name: &str) {
    let was_complete = crate::agent::coding_round_complete(context.state);
    let recorded = crate::agent::apply_phase_judgment(context.state, text);
    for evidence in &recorded {
        eprintln!(
            "evidence: at={} phase={} accepted=true source=phase_judge room={room_name}",
            log_clock(context.state),
            crate::agent::phase_id(evidence.phase),
        );
    }
    if recorded.is_empty() || context.state.ended {
        return;
    }
    let note = crate::agent::phase_judgment_note(context.state, &recorded, was_complete);
    if let Err(error) = send_model_context(
        context.gemini,
        context.state,
        ModelInputKind::Turn,
        &note,
        None,
    )
    .await
    {
        // A replacement socket's cold briefing carries the recorded steps and
        // any released follow-ups, so nothing here is owed.
        eprintln!("Gemini phase note failed ({error}); waiting for the close to be reported");
    }
}

/// Waits for one judgment until `deadline`, collecting it if it lands and
/// aborting it if it does not.
async fn await_phase_judgment(
    context: &mut GeminiEventContext<'_>,
    interview: InterviewContext<'_>,
    mut judgment: tokio::task::JoinHandle<String>,
    deadline: Instant,
) {
    match tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), &mut judgment).await {
        Ok(finished) => finish_phase_judgment(context, interview, finished).await,
        Err(_) => judgment.abort(),
    }
}

/// A judgment that has ended, however it ended: applied if it returned, and
/// otherwise a judgment that did not happen.
pub(super) async fn finish_phase_judgment(
    context: &mut GeminiEventContext<'_>,
    interview: InterviewContext<'_>,
    finished: Result<String, tokio::task::JoinError>,
) {
    match finished {
        Ok(text) => collect_phase_judgment(context, &text, interview.boot.room_name).await,
        Err(error) => {
            eprintln!("phase judge ended abnormally: {error}");

            // Answered with nothing, as a failed call is, so its window is read
            // again.
            crate::agent::apply_phase_judgment(context.state, "");
        }
    }
}

/// Brings the judged steps up to date before the end, after which the report
/// is written from the evidence. The judgment in flight is waited for, then
/// one more is asked for anything said since, within one shared bound.
///
/// Only the end waits in the room loop. Waiting stops the loop forwarding the
/// candidate's audio. Its queue holds about forty seconds (see
/// `candidate_audio_stream`), so speech meanwhile is delayed rather than lost,
/// but it reaches the interviewer only after the wait; at the end nothing more
/// is owed a reply, but the round transition is held instead, see
/// `defers_round_transition`.
pub(super) async fn settle_phase_judge(
    context: &mut GeminiEventContext<'_>,
    interview: InterviewContext<'_>,
) {
    let deadline = Instant::now() + PHASE_JUDGE_SETTLE;
    if let Some(judgment) = context.activity.phase_judge.take() {
        await_phase_judgment(context, interview, judgment, deadline).await;
    }
    if crate::agent::phase_judge_due(context.state)
        && Instant::now() < deadline
        && context.activity.reserve_phase_judge(Instant::now())
    {
        let judgment = spawn_phase_judge(context.state, interview);
        await_phase_judgment(context, interview, judgment, deadline).await;
    }
}

/// Whether a control packet is the end, which `settle_phase_judge` runs before.
pub(super) fn settles_phase_judge(
    state: &RuntimeState,
    topic: &str,
    payload: &serde_json::Value,
) -> bool {
    !state.ended && control_type(topic, payload) == Some("end_interview")
}

/// The `type` of a control packet, or none for any other topic.
pub(super) fn control_type<'a>(topic: &str, payload: &'a serde_json::Value) -> Option<&'a str> {
    (topic == TOPIC_CONTROL)
        .then(|| payload.get("type").and_then(serde_json::Value::as_str))
        .flatten()
}

/// Whether a round transition must wait for the phase judge. The transition
/// skips the behavioral round for an incomplete coding round and cannot be
/// taken back, so a judgment that may complete it, in flight or due for what
/// was just said, is let finish first. The packet is held rather than waited
/// on, and the watch tick applies it once the judgment is in or the hold runs
/// out; see `RuntimeActivity::parked_transition_ready`.
pub(super) fn defers_round_transition(
    state: &RuntimeState,
    judge_running: bool,
    topic: &str,
    payload: &serde_json::Value,
) -> bool {
    crate::agent::round_transition_due(state, payload)
        && !state.behavioral_round_started
        && control_type(topic, payload) == Some("round_transition")
        && (judge_running || crate::agent::phase_judge_due(state))
}

/// The one side call of a kind that may be in flight, an interim review or a
/// phase judgment, owned rather than let loose.
///
/// Two things a bare `tokio::spawn` got wrong. A task that panics sends no
/// result, so a loop tracking "a review is running" in a bool would believe one
/// forever and spend a single panic to disable every remaining pause in the
/// interview; asking the handle is the same question with no second copy of the
/// answer. And `JoinHandle` detaches on drop, so an interview that ended under
/// a review left a task holding a cloned API key and writing to a log nobody
/// was reading any more. `run_room` has several exits and none of them should
/// have to remember this, so the abort is the drop.
#[derive(Default)]
pub(super) struct SideCall(Option<tokio::task::JoinHandle<String>>);

impl SideCall {
    pub(super) fn is_running(&self) -> bool {
        self.0.is_some()
    }

    /// The handle once it has finished, leaving the slot empty.
    ///
    /// `is_finished` is true for a task that panicked as well as one that
    /// returned, and awaiting a handle that has finished does not block, so
    /// this is the one place a review leaves the slot, however it ended.
    pub(super) fn finished(&mut self) -> Option<tokio::task::JoinHandle<String>> {
        if self.0.as_ref()?.is_finished() {
            self.0.take()
        } else {
            None
        }
    }

    /// The handle, finished or not, leaving the slot empty.
    pub(super) fn take(&mut self) -> Option<tokio::task::JoinHandle<String>> {
        self.0.take()
    }

    pub(super) fn start(&mut self, handle: tokio::task::JoinHandle<String>) {
        if let Some(replaced) = self.0.replace(handle) {
            replaced.abort();
        }
    }
}

impl Drop for SideCall {
    fn drop(&mut self) {
        if let Some(handle) = self.0.take() {
            handle.abort();
        }
    }
}

/// The stretch to review, with the cursor moved past it.
///
/// Split from the spawn above so the part that can be wrong is reachable from a
/// test. Which lines a pause reads, and whether the pause after it reads the
/// ones since rather than the same ones again, is the whole behavior here; the
/// rest is an HTTP call.
///
/// The window is marked read here, before the call goes out, rather than when
/// one returns. A call that fails would otherwise hand the same stretch to the
/// next pause, which would then be reading old speech instead of the speech
/// since -- and the transcript reaches the final reviewer whole either way, so
/// a window nobody managed to summarize is not a window anybody lost.
pub(super) fn take_interim_review_window(
    state: &mut RuntimeState,
    boot: &RuntimeBootstrap<'_>,
) -> String {
    // Both halves bounded. The cursor only moves when a review actually fires,
    // so a stretch that never offers a quiet moment banks lines indefinitely,
    // and the editor is unbounded on the way in -- either one would otherwise
    // hand a call that has twelve seconds an input too large to read.
    let window = transcript_tail(
        &crate::agent::mark_unrecognized_turns(
            &state.delivery_transcript()[unreviewed_from(state)..],
        ),
        INTERIM_WINDOW_BYTES,
    );
    state.interim_transcript_lines = state.transcript.len();

    // The tail, not the whole list. Every review used to be sent every note
    // taken so far, which is prefill growing quadratically across a session to
    // defend against a repeat `record_interim_notes` already drops.
    let recent = state
        .interim_notes
        .len()
        .saturating_sub(INTERIM_CONTEXT_NOTES);
    let evidence = state
        .prompt_evidence(crate::agent::ViewFor::Interim)
        .join("\n");
    let code = if state.code.trim().is_empty() {
        // The cursor moves here too, because it records what the last review
        // showed the model rather than the last code it was sent. Left behind,
        // a candidate who clears the editor and then restores exactly what was
        // there before is reported as unchanged, to a model whose last look at
        // the editor found it empty.
        state.interim_code = state.code.clone();
        String::new()
    } else if state.code == state.interim_code {
        INTERIM_CODE_UNCHANGED.to_string()
    } else {
        state.interim_code = state.code.clone();
        code_head(&state.code, INTERIM_CODE_BYTES)
    };
    let prompt = interim_review_prompt(&InterimReviewInput {
        problem: boot.problem,
        interview_mode: state.interview_mode,
        example_drawing_enabled: state.example_drawing_enabled,
        transcript_window: &window,
        code: &code,
        language: &state.language,
        already_recorded: &state.interim_notes[recent..].join("\n"),
        evidence: &evidence,
    });
    state.evidence_ledger.record_model_input(
        ModelInputKind::Interim,
        &format!("{}\n\n{prompt}", crate::agent::interim_system_instruction()),
    );
    prompt
}
