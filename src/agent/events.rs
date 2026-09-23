//! Applying one data event from the browser to the interview state.
//!
//! Four topics, four appliers, and a dispatcher over them. They are together
//! because they share `RuntimeState` and the rule that a payload is a claim
//! rather than a fact: everything here is written by the candidate's browser,
//! so each applier decides what it is willing to believe before it stores it.

use super::{
    DataEventResult, INTERVIEWER_SPEAKER, InterviewLoop, LanguageChoiceContext,
    LifecycleTransition, MAX_INTEGRITY_EVENTS, ROUND_TRANSITION_SKEW, RuntimeState, TIME_WARNING_S,
    analyze_code, analyze_code_cached, behavioral_time_warning, changed_excerpt, cold_restart,
    format_test_run_for_reaction, integrity_hash, language_choice, observe_code,
    observe_code_cached, python_truthy, resume, round_skipped, round_started,
    sanitize_integrity_event, sanitize_test_run, spoken_language, test_reaction_decision,
    test_results_reaction, test_setup_error_reaction, time_warning,
};
use crate::runtime::{TOPIC_CODE_UPDATE, TOPIC_CONTROL, TOPIC_INTEGRITY, TOPIC_TEST_RESULTS};

/// Applies an event, stamping it with the server's clock.
///
/// The receipt is read here and never taken from the packet. `at` is the
/// browser's claim about when something happened, and letting it in through
/// this door made the candidate's machine the authority on when the server
/// received their work: the one timestamp in the ledger that is not a claim
/// would have been supplied by the party the ledger exists to observe. The old
/// fallback was worse in a quieter way, substituting the ledger's sequence
/// counter when `at` was absent, so a session mixed epoch milliseconds with
/// small integers and every latency computed across the two was nonsense.
pub fn apply_data_event(
    state: &mut RuntimeState,
    topic: &str,
    payload: &serde_json::Value,
    since_last_test_reaction_seconds: f64,
) -> DataEventResult {
    apply_data_event_at(
        state,
        topic,
        payload,
        since_last_test_reaction_seconds,
        crate::current_epoch_millis(),
    )
}

/// Applies an event with a receipt timestamp the caller already has. LiveKit
/// reads its clock once per packet so everything one packet produces shares a
/// reading, and a replay passes the recorded receipts to get the recorded
/// ledger back.
pub(crate) fn apply_data_event_at(
    state: &mut RuntimeState,
    topic: &str,
    payload: &serde_json::Value,
    since_last_test_reaction_seconds: f64,
    receipt_timestamp_ms: u64,
) -> DataEventResult {
    apply_data_event_at_with_received(
        state,
        topic,
        payload,
        since_last_test_reaction_seconds,
        receipt_timestamp_ms,
        true,
    )
}

pub(crate) fn apply_server_event_at(
    state: &mut RuntimeState,
    topic: &str,
    payload: &serde_json::Value,
    since_last_test_reaction_seconds: f64,
    receipt_timestamp_ms: u64,
) -> DataEventResult {
    apply_data_event_at_with_received(
        state,
        topic,
        payload,
        since_last_test_reaction_seconds,
        receipt_timestamp_ms,
        false,
    )
}

fn apply_data_event_at_with_received(
    state: &mut RuntimeState,
    topic: &str,
    payload: &serde_json::Value,
    since_last_test_reaction_seconds: f64,
    receipt_timestamp_ms: u64,
    received: bool,
) -> DataEventResult {
    if received {
        state.evidence_ledger.record_received_event();
    }
    if state.behavioral_round_started && matches!(topic, TOPIC_CODE_UPDATE | TOPIC_TEST_RESULTS) {
        return DataEventResult::default();
    }
    if state.paused && matches!(topic, TOPIC_CODE_UPDATE | TOPIC_TEST_RESULTS) {
        return DataEventResult::default();
    }
    let mut result = match topic {
        TOPIC_CODE_UPDATE => apply_code_update(state, payload, receipt_timestamp_ms),
        TOPIC_TEST_RESULTS => apply_test_results(
            state,
            payload,
            since_last_test_reaction_seconds,
            receipt_timestamp_ms,
        ),
        TOPIC_CONTROL => apply_control(state, payload, receipt_timestamp_ms),
        TOPIC_INTEGRITY => apply_integrity(state, payload),
        _ => DataEventResult::default(),
    };

    // Stamped here rather than in each arm, because the reading is the same
    // fact for all of them and an arm added later would otherwise be the one
    // stage direction that leaves the interviewer guessing again.
    result.generate_reply = result
        .generate_reply
        .map(|prompt| crate::agent::with_timer(state, prompt));
    result
}

/// The id and its spoken form, or nothing at all when the browser sent
/// something the tabs do not offer.
fn offered_language(language: &str) -> Option<(&str, &'static str)> {
    spoken_language(language).map(|spoken| (language, spoken))
}

fn apply_code_update(
    state: &mut RuntimeState,
    payload: &serde_json::Value,
    receipt_timestamp_ms: u64,
) -> DataEventResult {
    // A packet with no string `code` is a malformed packet, not an empty
    // editor, and it changes nothing. Defaulting to "" meant one of those wiped
    // the authoritative buffer, and the buffer is what the report is written
    // from; letting its language through filed the old buffer under a new
    // language, so `state.code` would no longer be the code of
    // `state.language`.
    let Some(new_code) = payload.get("code").and_then(serde_json::Value::as_str) else {
        return DataEventResult::default();
    };

    // Editor packets arrive several times a second and are usually identical to
    // the last one, so only touch the buffer when the text actually moved.
    let first_code_packet = state.code.is_empty();
    let update_last_code_change = new_code != state.code;
    let prior_code =
        update_last_code_change.then(|| std::mem::replace(&mut state.code, new_code.to_string()));

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
    //
    // `first_code_packet` asks whether the buffer was empty, which is the same
    // question only until the candidate clears the editor and starts over: that
    // leaves an empty buffer for one packet, and the keystroke after it is not
    // a template. Once the candidate has edited at all, an empty buffer is
    // their doing and what follows it is theirs too.
    //
    // A packet that carries a switch carries the new tab's buffer, and that
    // buffer belongs to the tab, not to the keystroke that would have been the
    // alternative reason to send it. Which of the two a packet is has to be
    // decided in the browser, because only the browser knows whether the text
    // it is holding is the starter it just loaded or that starter with typing
    // on top: the server sees a buffer that differs from the starter either
    // way, and would read a candidate returning to a tab they worked in earlier
    // as editing it again on every visit. `flushPendingLanguagePublish` in
    // web/interview.js is what keeps the two apart, by sending the switch
    // before the typing that followed it.
    let candidate_edit =
        update_last_code_change && switching.is_none() && (!first_code_packet || state.code_edited);
    if candidate_edit {
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

    // The starters come from the problem (`RuntimeState::for_problem`), never
    // from a packet. A language the problem has no starter for, which only a
    // state built without one meets, falls back to its first buffer.
    if !state.code_templates.contains_key(&state.language) {
        state
            .code_templates
            .insert(state.language.clone(), new_code.to_string());
    }

    // This follows the accepted language switch above: one packet can swap a
    // tab and its editor buffer, and the ledger must attach that buffer to the
    // new tab rather than to the language that happened to be active first.
    //
    // A switch counts even when it carried no new text. Two tabs can hold the
    // same buffer, and gating on the digest alone left the ledger describing
    // the tab the candidate had just left, with a parse read under the other
    // tab's grammar beside it. The reducer still coalesces the case where
    // neither the text nor the language moved.
    if update_last_code_change || language_changed.is_some() {
        let analysis = if language_changed.is_some() {
            // The buffer before this packet belongs to the tab the candidate
            // just left, so there is nothing here to diff against and no
            // classification to make. What there is to record is whether the
            // new tab's buffer parses, and recording it is not optional: the
            // recovery baseline below is only kept when the buffer before an
            // invalid draft was seen to parse, so calling the parser
            // unavailable for a language this server parses fine switched the
            // recovery off for the rest of the invalid streak.
            //
            // `last_parseable_code` survives the switch. It is keyed by
            // language and only consulted for a matching one, so a trip to
            // another tab and back is exactly the case it should recover from.
            observe_code_cached(&mut state.parse_cache, &state.language, new_code)
        } else {
            analyze_code_update(
                state,
                prior_code
                    .as_deref()
                    .expect("changed code has a prior buffer"),
                new_code,
            )
        };
        state.evidence_ledger.record_code_with_analysis(
            payload.get("at").and_then(serde_json::Value::as_u64),
            receipt_timestamp_ms,
            state.language.as_str(),
            new_code,
            candidate_edit,
            analysis,
        );
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

/// Compare a completed edit with the buffer that preceded an invalid draft.
/// The ledger deliberately retains no source, so this short-lived baseline
/// belongs to the runtime state and is discarded as soon as parsing recovers.
fn analyze_code_update(
    state: &mut RuntimeState,
    previous: &str,
    current: &str,
) -> super::CodeAnalysis {
    let language = state.language.clone();
    let previous_was_parsed =
        state.evidence_ledger.code.parser_observation == Some(super::CodeObservation::Parsed);
    let (analysis, current_alone) =
        analyze_code_cached(&mut state.parse_cache, &language, previous, current);
    if analysis.observation != super::CodeObservation::SyntaxInvalid {
        // Only this language's baseline is spent. Another tab's is still the
        // buffer that tab was last known to parse, and dropping it here is what
        // made a candidate who consulted a second tab mid-error unrecoverable.
        state.last_parseable_code.remove(&language);
        return analysis;
    }

    // Only a buffer that parses on its own can recover against the baseline,
    // and most of an invalid streak is typing that does not, so asking first
    // saves two parses per keystroke of it.
    if current_alone == super::CodeObservation::Parsed
        && let Some(baseline) = state.last_parseable_code.get(&language)
    {
        let recovered = analyze_code(&language, baseline, current);
        if recovered.observation == super::CodeObservation::Parsed {
            state.last_parseable_code.remove(&language);
            return recovered;
        }
    }

    // The pairwise analysis says `syntax_invalid` when either side fails to
    // parse, so the edit that repairs a broken draft is reported invalid along
    // with the draft it repaired. The baseline above recovers that whenever the
    // streak began from a buffer this server saw parse; when it did not -- an
    // invalid first packet, or a buffer restored into the editor -- there is
    // nothing to diff against, and the candidate's fix was recorded as another
    // invalid one. It never moved `semantic_revision`, so the watch loop's
    // significant-change gate never saw the moment the code started parsing.
    //
    // No classification is invented here. What is available without a baseline
    // is the observation a language switch records for the same reason, and for
    // the same reason it carries no classification: nothing was diffed.
    //
    // Semantic all the same, unlike that switch. A switch records a buffer that
    // did not move under a tab that did; this records a buffer that moved from
    // not being a program to being one, which is the change the watch loop is
    // waiting on. `record_code_with_analysis` is only reached when the digest
    // moved, so there is no text-unchanged case for this to overstate.
    //
    // Spelled out rather than `..observe_code(&language, current)`, which reads
    // better and re-parses the buffer `analyze_code_cached` parsed to produce
    // `current_alone`. That parse is the one this pair exists to remove.
    if current_alone == super::CodeObservation::Parsed {
        return super::CodeAnalysis {
            observation: super::CodeObservation::Parsed,
            classification: None,
            semantic_change: true,
            changed_nodes: Vec::new(),
        };
    }

    // Only once the buffer in hand is known not to parse either, so that the
    // baseline a streak is measured from is spent by the same rule the two
    // recoveries above spend it: a path that reports a parse leaves none
    // behind.
    if previous_was_parsed {
        state
            .last_parseable_code
            .insert(language, previous.to_string());
    }
    analysis
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
    receipt_timestamp_ms: u64,
) -> DataEventResult {
    // The browser timestamp is a claim stored only as evidence metadata. Read
    // it before sanitizing, because the prompt-facing test payload deliberately
    // drops fields it does not render.
    let source_timestamp_ms = payload.get("at").and_then(serde_json::Value::as_u64);

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
    state
        .evidence_ledger
        .record_test(source_timestamp_ms, receipt_timestamp_ms, payload);

    let decision = test_reaction_decision(state.ended, since_last_test_reaction_seconds);
    if !decision.react {
        return DataEventResult::default();
    }

    let setup_error = payload.get("setupError").is_some_and(python_truthy);
    let all_passed = payload
        .get("total")
        .and_then(serde_json::Value::as_i64)
        .is_some_and(|total| total > 0)
        && payload.get("passed").and_then(serde_json::Value::as_i64)
            == payload.get("total").and_then(serde_json::Value::as_i64);
    let summary = format_test_run_for_reaction(payload, state.test_runs);
    let excerpt = changed_excerpt(&state.language, &state.code_shown, &state.code);
    if excerpt.is_some() {
        state.code_shown = state.code.clone();
    }

    DataEventResult {
        update_last_test_reaction: true,
        update_last_interjection: true,
        generate_reply: Some(if setup_error {
            test_setup_error_reaction(&summary, excerpt.as_deref())
        } else {
            test_results_reaction(&summary, all_passed, excerpt.as_deref())
        }),
        ..DataEventResult::default()
    }
}

fn apply_control(
    state: &mut RuntimeState,
    payload: &serde_json::Value,
    receipt_timestamp_ms: u64,
) -> DataEventResult {
    match payload.get("type").and_then(serde_json::Value::as_str) {
        Some("pause_interview") if !state.ended => {
            control_pause(state, payload, receipt_timestamp_ms)
        }
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
            control_round_transition(state, receipt_timestamp_ms)
        }
        Some("time_warning")
            if !state.ended
                && !state.paused
                && !state.time_warning_seen
                && time_warning_is_due(state) =>
        {
            control_time_warning(state)
        }
        Some("end_interview") if !state.ended => {
            control_end_interview(state, payload, receipt_timestamp_ms)
        }
        _ => DataEventResult::default(),
    }
}

/// Pause used to be a practice-only affordance, and practice is gone. Left
/// working for everyone rather than deleted with the mode: a candidate whose
/// machine or network interrupts them mid-interview can at least stop the
/// interviewer talking into an empty room. It stops the conversation, not the
/// deadline, which runs on wall clock either way. It is recorded, so a paused
/// stretch is visible in the report.
fn control_pause(
    state: &mut RuntimeState,
    payload: &serde_json::Value,
    receipt_timestamp_ms: u64,
) -> DataEventResult {
    let paused = payload
        .get("paused")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    if paused == state.paused {
        return DataEventResult::default();
    }
    state.paused = paused;
    state.evidence_ledger.record_lifecycle(
        receipt_timestamp_ms,
        if paused {
            LifecycleTransition::Paused
        } else {
            LifecycleTransition::Resumed
        },
    );

    // A resumed interview whose interviewer was replaced mid-pause has to be
    // re-grounded before it is told to carry on: the fixed line below assumes a
    // Jim who remembers the conversation, and after a cold restart there is
    // none to continue from.
    let cold_brief = !paused && std::mem::take(&mut state.needs_cold_brief);
    DataEventResult {
        pause_changed: Some(paused),
        generate_reply: (!paused).then(|| {
            if cold_brief {
                state.code_shown = state.code.clone();
                cold_restart(state)
            } else {
                resume(state.behavioral_round_started)
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
fn control_round_transition(
    state: &mut RuntimeState,
    receipt_timestamp_ms: u64,
) -> DataEventResult {
    state.round_transition_seen = true;
    if super::coding_round_complete(state) {
        state.behavioral_round_started = true;
        state.behavioral_round_transcript_start = state.transcript.len();
        state.behavioral_round_prior_turn =
            super::last_speaker_line(&state.transcript, INTERVIEWER_SPEAKER)
                .map(|(index, line)| (index, line.clone()));
        state
            .evidence_ledger
            .record_lifecycle(receipt_timestamp_ms, LifecycleTransition::BehavioralStarted);
        DataEventResult {
            round_changed: Some("started"),
            generate_reply: Some(round_started()),
            ..DataEventResult::default()
        }
    } else {
        DataEventResult {
            round_changed: Some("skipped"),
            generate_reply: Some(round_skipped()),
            ..DataEventResult::default()
        }
    }
}

/// Whether the interview has actually run far enough to be nearly over.
///
/// The browser owns the countdown and the candidate owns the browser, so this
/// packet is a claim like any other from that side. Unchecked it was worse than
/// noise: accepting one at minute one both interrupts the candidate with a
/// warning that is not true and consumes `time_warning_seen`, so the real
/// five-minute warning is then refused for the rest of the interview. The
/// adjacent round transition has been validated against this clock all along;
/// this is the same check for the same reason.
fn time_warning_is_due(state: &RuntimeState) -> bool {
    let planned = u64::from(state.coding_minutes + state.behavioral_minutes) * 60;
    state.started_at.elapsed() + ROUND_TRANSITION_SKEW
        >= std::time::Duration::from_secs(planned.saturating_sub(TIME_WARNING_S))
}

/// The clock crossing the warning threshold, once.
///
/// The browser releases its own latch after a pause because its first packet
/// may have arrived while this side was paused. Remembering an accepted warning
/// here lets that retry through when needed while refusing it after it already
/// interrupted the candidate.
fn control_time_warning(state: &mut RuntimeState) -> DataEventResult {
    state.time_warning_seen = true;
    if !state.behavioral_round_started {
        super::skip_unassessed_star(state, "The five-minute cutoff prevented assessment.");
    }
    DataEventResult {
        generate_reply: Some(if state.behavioral_round_started {
            behavioral_time_warning()
        } else {
            time_warning()
        }),
        ..DataEventResult::default()
    }
}

/// The candidate leaving, however they left.
fn control_end_interview(
    state: &mut RuntimeState,
    payload: &serde_json::Value,
    receipt_timestamp_ms: u64,
) -> DataEventResult {
    // Only for a round that never opened. Once it has, a step left without a
    // row may be one the candidate declined, which stays unassessed rather than
    // skipped, and only the conversation can tell the two apart.
    if !state.behavioral_round_started {
        super::skip_unassessed_star(state, "The session ended before assessment.");
    }
    let prior_code = state.code.clone();
    let prior_language = state.language.clone();

    // Bound once, and read twice: here to take the buffer, and below to decide
    // whether the ledger has anything to record. `as_str` is already `None` for
    // a null, so a missing code and a null one are the same absence.
    let carried_code = (!state.behavioral_round_started)
        .then(|| payload.get("code").and_then(serde_json::Value::as_str))
        .flatten();
    if let Some(code) = carried_code {
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

    // Recorded only when the payload carried the buffer it ends on. The code
    // topic records a switch that brought no new text, but it never records one
    // that brought no buffer at all: a code packet without one is refused
    // outright. An end payload that names a language and carries no code has
    // said nothing about that tab's buffer, and filing the previous tab's code
    // under it would parse one language's buffer with another's grammar and
    // call the result an observation. `state.language` still moves, since the
    // report is written in the language the interview ended in; the ledger
    // keeps describing the last buffer it was actually given.
    if carried_code.is_some() && (state.code != prior_code || state.language != prior_language) {
        let current_code = state.code.clone();

        // A baseline exists only in the branch that reads it. It was computed
        // above the branch once, for both, and a term that could only matter on
        // the language-change path sat in it unread: that path never compares
        // against a baseline, so nothing could notice the term.
        let (candidate_edit, analysis) = if state.language != prior_language {
            // Same rule as the code topic: the final buffer arrived with a tab
            // switch, so it is observed on its own rather than diffed against
            // the language it replaced, and it is not the candidate's edit.
            (false, observe_code(&state.language, &current_code))
        } else {
            // Measured against the starter, not against the runtime's own
            // buffer. An interview can end before the first editor packet
            // lands, and the end payload then carries the template as the first
            // code this server has seen: comparing it with an empty buffer
            // reads the browser's starter as the candidate's work. A final edit
            // that never got its own packet still differs from the template, so
            // it still counts.
            let baseline = if state.code_edited {
                prior_code.clone()
            } else {
                state
                    .code_templates
                    .get(&state.language)
                    .cloned()
                    .unwrap_or_else(|| prior_code.clone())
            };

            // Diffed against the same baseline the attribution used. Handing
            // the empty runtime buffer to the parser instead reported every
            // token of the starter as added, so a one line edit that never got
            // its own packet arrived as a rewrite of the whole file.
            (
                current_code != baseline,
                analyze_code_update(state, &baseline, &current_code),
            )
        };
        state.evidence_ledger.record_code_with_analysis(
            payload.get("at").and_then(serde_json::Value::as_u64),
            receipt_timestamp_ms,
            state.language.as_str(),
            state.code.as_str(),
            candidate_edit,
            analysis,
        );
    }
    state.ended = true;
    state
        .evidence_ledger
        .record_lifecycle(receipt_timestamp_ms, LifecycleTransition::Ended);
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
