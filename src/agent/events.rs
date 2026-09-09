//! Applying one data event from the browser to the interview state.
//!
//! Four topics, four appliers, and a dispatcher over them. They are together
//! because they share `RuntimeState` and the rule that a payload is a claim
//! rather than a fact: everything here is written by the candidate's browser,
//! so each applier decides what it is willing to believe before it stores it.

use super::{
    DataEventResult, InterviewLoop, LanguageChoiceContext, MAX_INTEGRITY_EVENTS,
    ROUND_TRANSITION_SKEW, RuntimeState, cold_restart, format_test_run, integrity_hash, json_int,
    language_choice, python_truthy, sanitize_integrity_event, sanitize_test_run, spoken_language,
    spoken_minutes_from_remaining_seconds, test_reaction_decision, test_results_reaction,
    time_warning,
};
use crate::runtime::{TOPIC_CODE_UPDATE, TOPIC_CONTROL, TOPIC_INTEGRITY, TOPIC_TEST_RESULTS};

pub fn apply_data_event(
    state: &mut RuntimeState,
    topic: &str,
    payload: &serde_json::Value,
    since_last_test_reaction_seconds: f64,
) -> DataEventResult {
    if state.behavioral_round_started && matches!(topic, TOPIC_CODE_UPDATE | TOPIC_TEST_RESULTS) {
        return DataEventResult::default();
    }
    if state.paused && matches!(topic, TOPIC_CODE_UPDATE | TOPIC_TEST_RESULTS) {
        return DataEventResult::default();
    }
    match topic {
        TOPIC_CODE_UPDATE => apply_code_update(state, payload),
        TOPIC_TEST_RESULTS => apply_test_results(state, payload, since_last_test_reaction_seconds),
        TOPIC_CONTROL => apply_control(state, payload),
        TOPIC_INTEGRITY => apply_integrity(state, payload),
        _ => DataEventResult::default(),
    }
}

/// The id and its spoken form, or nothing at all when the browser sent
/// something the tabs do not offer.
fn offered_language(language: &str) -> Option<(&str, &'static str)> {
    spoken_language(language).map(|spoken| (language, spoken))
}

fn apply_code_update(state: &mut RuntimeState, payload: &serde_json::Value) -> DataEventResult {
    // A packet with no string `code` is a malformed packet, not an empty
    // editor. Defaulting to "" meant one of those wiped the authoritative
    // buffer, and the buffer is what the report is written from.
    let new_code = payload.get("code").and_then(serde_json::Value::as_str);

    // Editor packets arrive several times a second and are usually identical to
    // the last one, so only touch the buffer when the text actually moved.
    let first_code_packet = state.code.is_empty();
    let update_last_code_change = new_code.is_some_and(|code| code != state.code);
    if let Some(code) = new_code.filter(|_| update_last_code_change) {
        state.code.clear();
        state.code.push_str(code);
    }

    // A language switch is silent from the interviewer's side: the click swaps
    // the editor buffer and publishes the same code topic as any keystroke. The
    // greeting asks the candidate to pick one, so leaving the pick
    // unacknowledged makes the interviewer look like they missed it.
    //
    // Only a real change speaks. The browser publishes its starting language on
    // connect, which matches the default and must not produce a confirmation
    // for a choice nobody made. Only a language the tabs actually offer may
    // change the state or reach a prompt. The id comes from the candidate's
    // browser and is interpolated into Gemini's instructions, so an unvalidated
    // one is a way to write into them.
    let switching = payload
        .get("language")
        .and_then(serde_json::Value::as_str)
        .filter(|language| *language != state.language)
        .and_then(offered_language);

    // A non-empty buffer is not evidence that the candidate has done anything.
    // The browser publishes the starter template on connect and publishes the
    // next language's template on every switch, in the same packet as the
    // switch, so the buffer is a template from the first second of the
    // interview onward. What separates work from a template is a change that
    // arrived without a language switch, which is a keystroke.
    if update_last_code_change && !first_code_packet && switching.is_none() {
        state.code_edited = true;
    }

    let mut language_changed = None;
    if let Some(spoken) = switching {
        state.language = spoken.0.to_string();
        state.language_chosen = true;

        // Reading the buffer here instead asked Gemini not to make a candidate
        // "restate work they already completed" for a template they had not
        // touched, which skipped the restatement the whole REACTO opening is
        // built on for anyone who picked a language before speaking.
        let context = if state.code_edited {
            LanguageChoiceContext::SwitchWithCode
        } else {
            LanguageChoiceContext::Start
        };
        language_changed = Some(language_choice(spoken.1, context));
    }

    DataEventResult {
        update_last_code_change,

        // Counts as an interjection: it is the interviewer speaking unprompted,
        // and without this a candidate trying two tabs in a row is talked over
        // twice while the cooldown thinks nothing has been said.
        update_last_interjection: language_changed.is_some(),
        generate_reply: language_changed,
        ..DataEventResult::default()
    }
}

/// # What a passing run means
///
/// Nothing, on its own. `passed` and `total` are counted in the candidate's
/// browser against a judge the candidate's browser holds, and
/// `agent/integrity.rs` already says why that makes them an account rather than
/// evidence: the reporter is the adversary. Believing the count here is a
/// deliberate product choice, not an oversight. The interviewer reacts to what
/// the candidate says happened, the same way a human interviewer reacts to
/// "that one passes" without rerunning it.
///
/// So the numbers are safe to narrate and unsafe to score. What carries weight
/// is the code itself, which the agent receives over the code topic and can
/// read, and the room state it observes for itself. Anything that grades rather
/// than converses has to judge server-side, against code the server holds.
fn apply_test_results(
    state: &mut RuntimeState,
    payload: &serde_json::Value,
    since_last_test_reaction_seconds: f64,
) -> DataEventResult {
    // Bounded before it is stored, not before it is rendered. Both readers of
    // `last_test_run` put it in front of a model, so the sanitized value has to
    // be the only one that exists past this line.
    let payload = &sanitize_test_run(payload);

    // The sanitizer reports "no run happened" as null, and the caller has to
    // honor that or the refusal to invent a run is undone one line later:
    // counting it and reacting to it is what telling the report a run occurred
    // looks like from here.
    if payload.is_null() {
        return DataEventResult::default();
    }
    state.last_test_run = Some(payload.clone());
    state.test_runs += 1;

    let decision = test_reaction_decision(state.ended, since_last_test_reaction_seconds);
    if !decision.react {
        return DataEventResult::default();
    }

    let all_passed = payload
        .get("setupError")
        .is_none_or(|value| !python_truthy(value))
        && payload
            .get("total")
            .and_then(serde_json::Value::as_i64)
            .is_some_and(|total| total > 0)
        && payload.get("passed").and_then(serde_json::Value::as_i64)
            == payload.get("total").and_then(serde_json::Value::as_i64);
    let summary = format_test_run(Some(payload), state.test_runs);

    DataEventResult {
        update_last_test_reaction: true,
        update_last_interjection: true,
        generate_reply: Some(test_results_reaction(&summary, all_passed)),
        ..DataEventResult::default()
    }
}

fn apply_control(state: &mut RuntimeState, payload: &serde_json::Value) -> DataEventResult {
    match payload.get("type").and_then(serde_json::Value::as_str) {
        Some("pause_interview") if !state.ended => control_pause(state, payload),
        Some("round_transition")
            if !state.ended
                && !state.paused
                && state.interview_loop == InterviewLoop::CodingBehavioral
                && !state.round_transition_seen
                && payload.get("round").and_then(serde_json::Value::as_str)
                    == Some("behavioral")
                && state.started_at.elapsed() + ROUND_TRANSITION_SKEW
                    >= std::time::Duration::from_secs(u64::from(state.coding_minutes) * 60) =>
        {
            control_round_transition(state)
        }
        Some("time_warning") if !state.ended && !state.paused && !state.time_warning_seen => {
            control_time_warning(state, payload)
        }
        Some("end_interview") if !state.ended => control_end_interview(state, payload),
        _ => DataEventResult::default(),
    }
}

/// Pause used to be a practice-only affordance, and practice is gone. Left
/// working for everyone rather than deleted with the mode: a candidate whose
/// machine or network interrupts them mid-interview can at least stop the
/// interviewer talking into an empty room. It stops the conversation, not the
/// deadline, which runs on wall clock either way. It is recorded, so a paused
/// stretch is visible in the report.
fn control_pause(state: &mut RuntimeState, payload: &serde_json::Value) -> DataEventResult {
    let paused = payload
        .get("paused")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    if paused == state.paused {
        return DataEventResult::default();
    }
    state.paused = paused;

    // A resumed interview whose interviewer was replaced mid-pause has to be
    // re-grounded before it is told to carry on: the fixed line below assumes a
    // Jim who remembers the conversation, and after a cold restart there is
    // none to continue from.
    let cold_brief = !paused && std::mem::take(&mut state.needs_cold_brief);
    DataEventResult {
        pause_changed: Some(paused),
        generate_reply: (!paused).then(|| {
            if cold_brief {
                cold_restart(state)
            } else {
                "The interview has resumed. Continue with your REACTO step.".to_string()
            }
        }),

        // Resuming makes Jim speak, so it starts the interjection cooldown like
        // every other reply here. Without this the timing loop could follow the
        // resume line straight into a proactive review, talking twice over a
        // candidate who just came back.
        update_last_interjection: !paused,
        ..DataEventResult::default()
    }
}

/// The reserved behavioral round, opened or refused, once.
fn control_round_transition(state: &mut RuntimeState) -> DataEventResult {
    state.round_transition_seen = true;
    if super::coding_round_complete(state) {
        state.behavioral_round_started = true;
        DataEventResult {
            round_changed: Some("started"),
            generate_reply: Some("[SYSTEM EVENT] The trusted coding completion gate passed: Test and Optimizations both have candidate evidence. The coding round is closed. Begin the reserved behavioral round now with exactly one concise question under the private STAR, profile, and document-grounding policies. Use prior candidate answers only to deepen the follow-up; do not repeat them and do not return to coding.".to_string()),
            ..DataEventResult::default()
        }
    } else {
        DataEventResult {
            round_changed: Some("skipped"),
            generate_reply: Some("[SYSTEM EVENT] The behavioral reserve began, but the trusted coding completion gate did not pass because Test or Optimizations evidence is absent. Do not start STAR. Keep the candidate focused on a testable solution, highest-value tests, and justified complexity until the session ends; missing STAR phases will be marked skipped.".to_string()),
            ..DataEventResult::default()
        }
    }
}

/// The clock crossing the warning threshold, once.
///
/// The browser releases its own latch after a pause because its first packet
/// may have arrived while this side was paused. Remembering an accepted warning
/// here lets that retry through when needed while refusing it after it already
/// interrupted the candidate.
fn control_time_warning(state: &mut RuntimeState, payload: &serde_json::Value) -> DataEventResult {
    state.time_warning_seen = true;
    let remaining_seconds = payload
        .get("remainingSeconds")
        .and_then(json_int)
        .unwrap_or(300);
    let minutes = spoken_minutes_from_remaining_seconds(remaining_seconds) as u32;
    DataEventResult {
        generate_reply: Some(if state.behavioral_round_started {
            format!(
                "[SYSTEM EVENT] Exactly {minutes} minutes remain in the active behavioral round. Do not return to coding or ask a new question. Let the candidate finish the current answer, ask at most the one permitted neutral missing-STAR follow-up, then close naturally."
            )
        } else {
            time_warning(minutes)
        }),
        ..DataEventResult::default()
    }
}

/// The candidate leaving, however they left.
fn control_end_interview(state: &mut RuntimeState, payload: &serde_json::Value) -> DataEventResult {
    if !state.behavioral_round_started
        && !payload.get("code").is_some_and(serde_json::Value::is_null)
        && let Some(code) = payload.get("code").and_then(serde_json::Value::as_str)
    {
        state.code = code.to_string();
    }

    // Read independently of the code. Nesting it meant a payload whose code was
    // absent or null also discarded the language, and the report prompt was
    // then written against whatever language the session last happened to
    // record. Validated for the same reason as the code path: this string
    // reaches the report prompt.
    if !state.behavioral_round_started
        && let Some((id, _)) = payload
            .get("language")
            .and_then(serde_json::Value::as_str)
            .and_then(offered_language)
    {
        state.language = id.to_string();
    }
    state.ended = true;
    DataEventResult {
        finish_interview: Some(
            payload
                .get("reason")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("candidate_ended")
                .to_string(),
        ),
        ..DataEventResult::default()
    }
}

fn apply_integrity(state: &mut RuntimeState, payload: &serde_json::Value) -> DataEventResult {
    let Some(event) = sanitize_integrity_event(payload) else {
        return DataEventResult::default();
    };
    let seq = event.get("seq").and_then(serde_json::Value::as_u64);
    let prev_hash = event.get("prevHash").and_then(serde_json::Value::as_str);
    let (expected_seq, expected_prev_hash) = state
        .integrity_chain
        .as_ref()
        .map_or((1, ""), |(seq, hash)| {
            (seq.saturating_add(1), hash.as_str())
        });

    let computed_hash = integrity_hash(&event);

    // Named, because a rejection here is permanent. Sequence continuity is
    // required, so `expected_seq` never advances past a refused event and every
    // later one is refused for the same reason. Four different conditions used
    // to return the same silent default, and a hash mismatch caused by this
    // process normalizing `detail` before rehashing it emptied the evidence
    // section of every camera interview without a line anywhere saying so.
    //
    // Only tampering belongs here. A retention decision is handled below, after
    // the cursor has moved, because punishing the producer for this process
    // running out of room is how a full buffer used to end the chain.
    let refusal = if seq != Some(expected_seq) {
        Some(format!("sequence expected {expected_seq}, got {seq:?}"))
    } else if prev_hash != Some(expected_prev_hash) {
        Some("previous hash does not match the last accepted event".to_string())
    } else if event.get("hash").and_then(serde_json::Value::as_str) != Some(computed_hash.as_str())
    {
        Some("hash mismatch; the producer and this verifier disagree".to_string())
    } else {
        None
    };
    if let Some(reason) = refusal {
        eprintln!(
            "WARNING: integrity event refused, chain stops here: {reason} (seq={seq:?}, type={})",
            event
                .get("type")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("?")
        );
        return DataEventResult::default();
    }

    // The chain is satisfied, so the cursor moves whatever happens to the event
    // itself. Everything past this line is about what is worth keeping, and
    // none of it can refuse the next event.
    //
    // Built from the values the checks above already proved equal, rather than
    // re-read from the event. A conditional here would have a branch that
    // silently leaves the cursor behind, which is the failure this separation
    // exists to remove.
    state.integrity_chain = Some((expected_seq, computed_hash));

    // Liveness, not evidence, and now stored as such. Keeping only the ends of
    // the run is what lets the evidence buffer be a plain queue again.
    if is_heartbeat(&event) {
        if state.integrity_first_heartbeat.is_none() {
            state.integrity_first_heartbeat = Some(event);
        } else {
            state.integrity_last_heartbeat = Some(event);
        }
        return DataEventResult::default();
    }

    if !valid_review_sources(state, &event) {
        eprintln!(
            "WARNING: integrity event dropped from the evidence, chain continues: review event \
             references an event no longer retained (seq={seq:?})"
        );
        return DataEventResult::default();
    }

    state.integrity_events.push(event);

    // Plain oldest-out, because nothing periodic lands here any more. This used
    // to hold heartbeats too, and at one every five seconds they filled all 25
    // slots in about two minutes: a camera that stopped at minute three was
    // gone from a thirty minute report by minute five.
    if state.integrity_events.len() > MAX_INTEGRITY_EVENTS {
        state.integrity_events.remove(0);
    }
    DataEventResult::default()
}

/// Liveness rather than evidence, which is what sends it to its own field
/// instead of competing for a slot in the buffer.
fn is_heartbeat(event: &serde_json::Value) -> bool {
    event.get("type").and_then(serde_json::Value::as_str) == Some("INTEGRITY_HEARTBEAT")
}

fn valid_review_sources(state: &RuntimeState, event: &serde_json::Value) -> bool {
    if event.get("type").and_then(serde_json::Value::as_str) != Some("REVIEW_EVENT") {
        return true;
    }
    let Some(ids) = event
        .get("sourceEventIds")
        .and_then(serde_json::Value::as_array)
    else {
        return false;
    };
    if ids.is_empty() {
        return false;
    }
    ids.iter().all(|id| {
        let Some(seq) = id.as_str().and_then(|value| value.parse::<u64>().ok()) else {
            return false;
        };
        state
            .integrity_events
            .iter()
            .any(|stored| stored.get("seq").and_then(serde_json::Value::as_u64) == Some(seq))
    })
}
