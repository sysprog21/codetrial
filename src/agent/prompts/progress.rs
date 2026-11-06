//! What the interviewer is told as the candidate works: the greeting and
//! language, the follow-ups, the coding-progress family and the reactions to a
//! test run.

use super::material::numbered_list;
use crate::agent::{
    FrameworkEvidence, FrameworkPhase, InterviewLoop, InterviewMode, REACTO_PHASE_IDS,
    RuntimeState, STAR_PHASE_IDS, coding_continues_past_gate, coding_round_complete,
    framework_progress, phase_id, phases_evidenced,
};
use crate::runtime::AGENT_NAME;

/// One opening per surface, and neither names the problem: the title and brief
/// are already in THE EXERCISE, which every turn is billed on, and repeated
/// here they stayed in the context and were billed on every turn a second time.
pub fn greeting(mode: InterviewMode) -> String {
    if mode.is_whiteboard() {
        // No language question: a whiteboard interview has no tabs to click and
        // nothing to compile, so asking would open the session with a decision
        // the candidate cannot act on. The restatement that the coding greeting
        // defers until after the language choice is therefore the first thing
        // asked here.
        return format!(
            "[SYSTEM EVENT] The interview starts now. This one is held at a whiteboard: there is no editor and nothing will run. Greet the candidate in at most four short sentences: introduce yourself as {AGENT_NAME}; introduce THE EXERCISE in one sentence in its scenario's own terms, without naming any published problem, practice site, or the technique it needs; say that you can see their board and will be watching it as they draw; and mention that they may ask for a hint if they get stuck. Do not volunteer a constraint, edge case, or hint, and do not read the scenario out word for word. Then ask them to restate the inputs, outputs, constraints, and ambiguities in their own words, and to ask whatever they need to pin down."
        );
    }
    format!(
        "[SYSTEM EVENT] The interview starts now. Greet the candidate in at most four short sentences: introduce yourself as {AGENT_NAME}; introduce THE EXERCISE in one sentence in its scenario's own terms, without naming any published problem, practice site, or the technique it needs; ask which programming language they would like to use; and tell them they can either say it or click the language tabs above the editor. Mention that they can switch at any time and may ask for a hint if they get stuck. Do not list the available languages aloud, do not volunteer a constraint, edge case, or hint, and do not read the scenario out word for word. After they choose a language, begin by asking them to restate the inputs, outputs, constraints, and ambiguities in their own words, and to ask whatever they need to pin down."
    )
}

/// The coding opening when the candidate chose a language before joining.
///
/// `spoken` comes from `spoken_language`, never from the packet itself, so the
/// allowlist that guards `language_choice` guards this prompt too.
pub fn greeting_with_language(spoken: &str) -> String {
    format!(
        "[SYSTEM EVENT] The interview starts now. The candidate chose {spoken} before joining, and it is already selected in the editor. Greet the candidate in at most four short sentences: introduce yourself as {AGENT_NAME}; introduce THE EXERCISE in one sentence in its scenario's own terms, without naming any published problem, practice site, or the technique it needs; and mention that they can switch languages with the tabs above the editor at any time and may ask for a hint if they get stuck. Do not ask which language they want, do not comment on whether {spoken} is a good choice, do not volunteer a constraint, edge case, or hint, and do not read the scenario out word for word. Then ask them to restate the inputs, outputs, constraints, and ambiguities in their own words, and to ask whatever they need to pin down."
    )
}

/// How a language id is said out loud, and the allowlist of ids the tabs offer.
///
/// `None` for anything else, and that is a boundary rather than a convenience.
/// The id arrives in a data packet the candidate's browser sends, and it is
/// interpolated into a prompt this process hands to Gemini, so passing an
/// unknown value through would let a modified client write text straight into
/// the interviewer's instructions. It also stops the interviewer announcing a
/// language the editor cannot run.
pub fn spoken_language(language: &str) -> Option<&'static str> {
    match language {
        "python" => Some("Python"),
        "javascript" => Some("JavaScript"),
        "c" => Some("C"),
        "cpp" => Some("C++"),
        "java" => Some("Java"),
        _ => None,
    }
}

/// Spoken when the candidate picks a language by clicking, which is silent from
/// the interviewer's side: the click changes the editor and nothing else, so
/// without this the candidate gets no acknowledgement that Jim noticed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LanguageChoiceContext {
    Start,
    SwitchWithCode,
    Continue,
}

pub fn language_choice(spoken: &str, context: LanguageChoiceContext) -> String {
    let next = match context {
        LanguageChoiceContext::Start => {
            "Then begin the interview by asking them to restate the inputs, outputs, constraints, and ambiguities in their own words."
        }
        LanguageChoiceContext::Continue => {
            "Continue the current discussion with its existing context and progress. If they have not yet begun discussing the exercise, ask them to restate the inputs, outputs, constraints, and ambiguities in their own words. Do not restart the interview or ask them to repeat inputs, outputs, constraints, examples, or an approach already discussed."
        }
        LanguageChoiceContext::SwitchWithCode => {
            "They already have code in the editor, so acknowledge the switch without restarting the interview or asking them to restate work they already completed."
        }
    };
    format!(
        "[SYSTEM EVENT] The candidate just selected {spoken} using the language tabs. In one short sentence, confirm you have seen it by name. {next} Do not restate the problem, suggest an approach, or comment on whether {spoken} is a good choice."
    )
}

/// The follow-ups, handed over when the coding round completes rather than held
/// in the live prompt from the first turn: they are several hundred characters
/// the model carries on every turn before it may use them, and a model holding
/// them from the start is a model that can raise one early.
fn follow_ups_text(lead: &str, limit: &str, follow_ups: &[&str]) -> String {
    format!(
        "{lead}, at most two of these, in order and one at a time, as a change to the scenario: discussion, not a second task{limit}:\n{}",
        numbered_list(follow_ups)
    )
}

/// The follow-ups the interviewer may now use, or none: the coding round is not
/// complete, or the problem has none to give. One rule for the evidence reply
/// that releases them and the cold restart that hands them over again.
pub fn released_follow_ups(state: &RuntimeState) -> Option<String> {
    (coding_round_complete(state) && !state.follow_ups.is_empty()).then(|| {
        if state.interview_loop == InterviewLoop::CodingOnly {
            follow_ups_text(
                "Follow-ups you may raise only after unresolved failures are addressed",
                "",
                state.follow_ups,
            )
        } else {
            follow_ups_text(
                "The coding round is complete. Follow-ups you may now raise",
                ", and never at the cost of the behavioral round or wrap-up",
                state.follow_ups,
            )
        }
    })
}

/// The earlier steps of the same framework that have no evidence yet, named
/// back to the interviewer when it banks a later one, or none.
///
/// The candidate's checklist is ticked from evidence alone, and the spoken
/// steps have nothing that makes the model call the tool while it is talking:
/// Coding is reached through `read_editor`, Repeat through nothing. So the list
/// showed Coding ticked with Repeat, Example and Algorithm still open until the
/// model got round to them, sometimes at the end. Asked here, in the reply to
/// the call that exposed the gap, the model catches up in the same turn. It is
/// told to record only what the candidate did, because an open step can also be
/// one the candidate skipped, and that one has to stay open.
///
/// Any row closes a step, a skip included. `framework_progress` leaves skips
/// out because they tick nothing, but a step the five-minute warning closed out
/// as skipped is not missing. Each step is named once for the same reason the
/// reply tells the model to record nothing for a skip: a step the candidate
/// jumped over stays open, and naming it again on every later step leaves
/// inventing the evidence as the only way to make it stop.
pub fn unrecorded_earlier_phases(
    state: &mut RuntimeState,
    evidence: &FrameworkEvidence,
) -> Option<String> {
    let phase = phase_id(evidence.phase);
    let ids: &[&'static str] = if REACTO_PHASE_IDS.contains(&phase) {
        &REACTO_PHASE_IDS
    } else {
        &STAR_PHASE_IDS
    };
    let at = ids.iter().position(|id| *id == phase)?;
    let missing = ids[..at]
        .iter()
        .filter(|id| {
            !state
                .framework_evidence
                .iter()
                .any(|item| phase_id(item.phase) == **id)
        })
        .filter(|id| {
            !state.earlier_steps_named.contains(id) && !state.earlier_steps_pending.contains(id)
        })
        .copied()
        .collect::<Vec<_>>();
    if missing.is_empty() {
        return None;
    }
    state.earlier_steps_pending.extend(&missing);
    Some(format!(
        "Unrecorded earlier step(s): {}. If the candidate already did one, record it silently with your next evidence call; if they skipped it, record nothing. Do not reopen earlier questions; respond to the newest unanswered item.",
        missing.join(", ")
    ))
}

/// Said wherever a missing evidence row could read as unfinished work. The
/// rows are the interviewer's own bookkeeping, and a step the candidate
/// covered but the model never recorded is still covered.
///
/// Complexity and edge cases only: Test follows the separate run or hand-trace
/// policy, rather than treating any mention of testing as completion.
const RECORD_UNRECORDED: &str = "If the conversation shows they already covered complexity or edge cases, do not ask them to repeat it; the platform records it from what they said.";
const BOARD_RECORD_UNRECORDED: &str = "If the conversation shows they already covered complexity or edge cases, record that evidence silently instead of asking them to repeat it.";

/// `RECORD_UNRECORDED` where the surface is not known to be the editor's.
pub(super) fn record_unrecorded(mode: InterviewMode) -> &'static str {
    if crate::agent::evidence_tool_declared(mode) {
        BOARD_RECORD_UNRECORDED
    } else {
        RECORD_UNRECORDED
    }
}

/// How a credited test run's code compares, by the server on the code the
/// browser submitted; see `RuntimeState::tested_code` and
/// `RuntimeState::analysis`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SincePrevious {
    /// Still the previous credited run's code, by the Test gate's rule.
    Unchanged,
    /// Changed enough since the complexity analysis was recorded that it may
    /// no longer describe the code: see `rewritten`.
    Rewritten,
    /// Neither: no earlier credited run, or a change the analysis still
    /// covers. The reaction treats these alike, so they are one case.
    Other,
}

/// What the server can say about the code since the latest test run that
/// executed it, or `None` when no run of the language on screen is recorded.
/// Judged by the same rule the Test gate uses, so the prompts cannot call code
/// unchanged that the gate would refuse to credit, or the reverse.
fn code_since_latest_run(state: &RuntimeState) -> Option<bool> {
    state
        .tested_code
        .as_ref()
        .filter(|tested| tested.language == state.language)
        .map(|_| crate::agent::tested_code_is_current(state))
}

const DISABLED_EXECUTION_POLICY: &str = "Code execution is disabled for this interview. Verify written code with hand traces, without asking for a run or claiming tests executed.";

const CODING_ONLY_ENDING: &str = "A coding-only interview ends when the platform timer expires or the candidate chooses End, even when all REACTO steps have evidence and all tests pass. Do not say goodbye or ask them to end the session.";
const WHITEBOARD_ONLY_ENDING: &str = "A whiteboard-only interview ends when the platform timer expires or the candidate chooses End, even when all REACTO steps have evidence. Do not say goodbye or ask them to end the session.";

pub(super) fn coding_only_ending(state: &RuntimeState) -> &'static str {
    if state.interview_mode.is_whiteboard() {
        WHITEBOARD_ONLY_ENDING
    } else {
        CODING_ONLY_ENDING
    }
}

/// The outcome of the credited run when the editor still holds exactly the code
/// it executed, and `None` when that run cannot speak for the code on screen.
pub(super) fn verified_outcome(state: &RuntimeState) -> Option<bool> {
    state
        .tested_passed
        .filter(|_| crate::agent::tested_code_is_exact(state))
}

pub(super) fn coding_only_continuation(state: &RuntimeState) -> String {
    // Every sentence below is about a run, and a whiteboard never has one: the
    // round is past its gate on the candidate's trace and their named cases,
    // which is all it will ever have.
    if state.interview_mode.is_whiteboard() {
        let offer = if state.follow_ups.is_empty() {
            "allow discussion of"
        } else {
            "offer the released follow-ups or discuss"
        };
        return format!(
            "{WHITEBOARD_ONLY_ENDING} Nothing runs at a whiteboard, so do not ask for a run; {offer} trade-offs or cases not yet covered, without starting a second task."
        );
    }
    if state.code_execution_disabled {
        return format!(
            "{CODING_ONLY_ENDING} {DISABLED_EXECUTION_POLICY} Do not repeat traces or questions already covered; discuss only unresolved concerns."
        );
    }
    let current = code_since_latest_run(state);
    let verified = verified_outcome(state);
    let unavailable = crate::agent::runner_unavailable_on_screen(state);
    let next = match (verified, current) {
        (Some(passed), _) => {
            if !passed {
                "The credited run executed the code on screen and has failing cases. Let the candidate finish editing, then ask them to diagnose one unresolved failing case or verify their fix; do not supply the bug or solution.".to_string()
            } else {
                let offer = if state.follow_ups.is_empty() {
                    "allow discussion of"
                } else {
                    "offer the released follow-ups or discuss"
                };
                format!("The credited run executed the code on screen and all cases passed. Do not ask for another run or repeat completed questions; {offer} trade-offs not yet covered, without starting a second task.")
            }
        }
        _ if unavailable => {
            "The runner cannot provide tests for this language. Let the candidate finish their work and discuss any unresolved concern using a hand trace; do not repeat a trace already covered.".to_string()
        }
        (None, Some(true)) => {
            "The code has a small edit since the credited run, so that run's result may not describe the code on screen. If the edit could change the result, ask them to click Run on the code now on screen; do not assume the earlier failures or passes still hold.".to_string()
        }
        (None, Some(false)) => {
            "The code has changed since the latest executed run. Let the candidate finish editing, then verify the changed code if the change could affect the result; do not assume earlier failures or passes describe it.".to_string()
        }
        _ => {
            "No executed run can be matched to the code on screen. Let the candidate finish editing, then invite them to click Run before drawing conclusions from earlier test results.".to_string()
        }
    };
    let outage = if unavailable && verified.is_some() {
        " The runner is currently unavailable; use a hand trace for any further verification, without repeating a trace already covered."
    } else {
        ""
    };
    format!("{CODING_ONLY_ENDING} {next}{outage}")
}

/// Where the coding round stands on the steps #66 kept sending candidates back
/// to, for every coding prompt that might otherwise ask for them again. `None`
/// when nothing is known: no real test run and no Test or Optimizations
/// evidence.
///
/// The pass counts are left to the evidence lines and the test report, which
/// already carry them. What this adds is what neither says: whether the code
/// changed since the run, compared here rather than left to a model that
/// cannot see the code the run tested, and that covered work is recorded
/// rather than asked for again.
pub(super) fn coding_progress(state: &RuntimeState) -> Option<String> {
    let progress = coding_progress_without_presence(state);
    let Some(presence) = candidate_code_presence(state) else {
        return progress;
    };
    Some(match progress {
        Some(progress) => format!("{presence} {progress}"),
        None => presence.to_string(),
    })
}

/// Only until Coding is evidenced: after that the progress text already says
/// the code exists, and the note would be resent on every review and nudge.
fn candidate_code_presence(state: &RuntimeState) -> Option<&'static str> {
    candidate_code_presence_given(state, crate::agent::code_written(state))
}

/// `candidate_code_presence` for a caller that has already compared the editor
/// with its starter, which is the one expensive step here.
pub(super) fn candidate_code_presence_given(
    state: &RuntimeState,
    written: bool,
) -> Option<&'static str> {
    (written
        && !state.interview_mode.is_whiteboard()
        && !state.behavioral_round_started
        && !phases_evidenced(state, &[FrameworkPhase::Coding]))
    .then_some("Current editor has candidate edits, not proof of a solution. Review them; do not ask to begin code that is already implemented.")
}

pub(super) fn coding_progress_without_presence(state: &RuntimeState) -> Option<String> {
    if state.code_execution_disabled
        && !state.interview_mode.is_whiteboard()
        && !coding_continues_past_gate(state)
    {
        let next = if coding_round_complete(state) {
            "Test and Optimizations have evidence; do not repeat completed traces, complexity or edge-case questions."
        } else if phases_evidenced(state, &[FrameworkPhase::Test]) {
            "Test has evidence from a hand trace; ask only for steps they have not covered."
        } else {
            "When written code is ready, ask for one ordinary and one boundary case with expected results; the platform records Test from that trace."
        };
        return Some(format!(
            "{DISABLED_EXECUTION_POLICY} {next} {RECORD_UNRECORDED}"
        ));
    }
    if coding_continues_past_gate(state) {
        return Some(format!(
            "Test and Optimizations have evidence; that records work attempted, not a passing solution. {}",
            coding_only_continuation(state)
        ));
    }
    let run = state
        .last_test_run
        .as_ref()
        .filter(|run| crate::agent::real_test_run(run));
    let unchanged = code_since_latest_run(state);
    if coding_round_complete(state) {
        let changed = if run.is_some() && unchanged == Some(false) {
            " The code differs from the credited run; revisit only what that change affects."
        } else {
            ""
        };
        return Some(format!(
            "The Test and Optimizations steps are done: do not ask them to run tests again or repeat complexity or edge-case questions already answered.{changed}"
        ));
    }
    let analysed = phases_evidenced(state, &[FrameworkPhase::Optimizations]);
    let Some(run) = run else {
        // No browser run, but the complexity may already be on record. Test is
        // not: only a run of the code, or a trace while the runner is down,
        // completes it, so asking to test is still right.
        return analysed.then(|| {
            "They have already covered the complexity; do not ask for it again.".to_string()
        });
    };
    let tested = phases_evidenced(state, &[FrameworkPhase::Test]);
    let failing = run.get("passed").and_then(serde_json::Value::as_i64)
        != run.get("total").and_then(serde_json::Value::as_i64);

    // The Test gate reads the same comparison: until Test is recorded, code
    // changed past the run, or never credited, still needs a run of what is on
    // screen, so the run is asked for rather than left to judgement. A run of
    // this code that still fails is a reason to diagnose, not to run it again.
    let rerun = match (unchanged, tested) {
        (Some(true), _) if crate::agent::latest_test_report_is_exact(state) => {
            let current = if failing {
                "The latest test run executed the code on screen and some cases still fail: help them diagnose it rather than asking them to run the same code again"
            } else {
                "The latest test run executed the code on screen, so do not ask them to run tests again"
            };
            format!("{current}.")
        }
        (Some(true), _) => {
            "Test credit is retained from an earlier run, but the latest browser report does not establish the current code's outcome. Do not assume earlier failures or passes still hold; ask for verification only if an unresolved change could affect the result.".to_string()
        }
        (Some(false) | None, false) => {
            "No credited run covers the code now on screen, so only a qualifying run of candidate-written code can complete Test.".to_string()
        }
        (Some(false), true) => {
            "The code differs from the credited run, so ask for a rerun only if the change could affect the result; do not assume earlier failures or passes still hold.".to_string()
        }
        (None, true) => {
            "They have run the tests; ask for a rerun only if the code may have changed in a way that affects the result.".to_string()
        }
    };
    Some(format!("{rerun} {RECORD_UNRECORDED}"))
}

/// The framework steps evidenced among `ids`, spelled out when there are none:
/// a blank list reads as a sentence cut short.
pub(crate) fn evidenced_among(state: &RuntimeState, ids: &[&str]) -> String {
    let phases = framework_progress(state)
        .into_iter()
        .filter(|phase| ids.contains(phase))
        .collect::<Vec<_>>();
    if phases.is_empty() {
        "none".to_string()
    } else {
        phases.join(", ")
    }
}

/// The code a test reaction carries: the change since the model last saw the
/// editor, or nothing when it has not changed. A run is the moment the
/// interviewer most needs the code it is reacting to, and without it the
/// reaction was a `read_editor` round trip before the candidate heard a word.
fn reaction_code(excerpt: Option<&str>) -> String {
    excerpt
        .map(|excerpt| format!("Their code, numbered:\n{excerpt}\n"))
        .unwrap_or_default()
}

/// What a test reaction says about recording Test, as the evidence gate would
/// answer for this run right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TestRecord {
    /// The run executed the code on screen and the platform records Test.
    Record,
    /// The run would have counted, but the candidate kept editing while it
    /// was in flight, so only a run of the code now on screen can.
    RunAgain,
    /// Test is already recorded, or this run could never have counted.
    Settled,
}

/// A run that earned no execution credit: no code with it, starter code, an
/// empty run, or a language the editor has left. Its counts describe nothing
/// the gate can match, so they never pick the next step. `recorded_earlier` is
/// the platform recording Test from an earlier run that still covers the
/// editor, which this run's arrival found unrecorded.
pub fn uncredited_test_results_reaction(
    summary_text: &str,
    excerpt: Option<&str>,
    state: &RuntimeState,
    recorded_earlier: bool,
) -> String {
    let code = reaction_code(excerpt);
    let next = if coding_continues_past_gate(state) {
        coding_only_continuation(state)
    } else if recorded_earlier {
        "An earlier run of the code on screen counts as testing, and the platform recorded Test from it. Say nothing about this run unless the candidate asks.".to_string()
    } else if phases_evidenced(state, &[FrameworkPhase::Test]) {
        "Continue from the conversation; do not choose the next step from this run.".to_string()
    } else {
        "Only a run that executes the code on screen can complete Test; if they want to test, ask them once to click Run.".to_string()
    };
    format!(
        "[SYSTEM EVENT] These test results provide no new execution evidence for the code on screen:\n{summary_text}\n{code}Do not treat these results as passing or failing, or narrate their counts. {next}"
    )
}

/// The reaction asks for nothing the evidence gate would refuse: a run the
/// candidate edited past is not called testing, and is not recorded.
///
/// A passing run used to open Optimizations every time, so a rerun, or a
/// result landing after the candidate had already given complexity and edge
/// cases, asked for them again. `since_previous` compares this run's code with
/// the previous credited run's.
pub fn test_results_reaction(
    summary_text: &str,
    all_passed: bool,
    record: TestRecord,
    excerpt: Option<&str>,
    state: &RuntimeState,
    since_previous: SincePrevious,
) -> String {
    let code = reaction_code(excerpt);
    if all_passed && record == TestRecord::RunAgain {
        // Its own reply rather than a clause: "run again" beside "move to
        // Optimizations" under a two-sentence cap reads as the second, and Test
        // stays open behind a wrap-up that is then refused.
        return format!(
            "[SYSTEM EVENT] The candidate just ran the built-in test cases and every one passed, but the editor has changed since this run:\n{summary_text}\n{code}Reported, not proof. This run cannot complete Test. In one short sentence, acknowledge it and ask them to click Run on the code now on screen; do not move to Optimizations until that run's results arrive."
        );
    }
    let record = match record {
        TestRecord::Record if all_passed => {
            " The platform recorded Test from this run; do not duplicate that evidence."
        }
        TestRecord::Record => {
            " A failing run still counts as testing. The platform recorded Test from this run; do not duplicate that evidence."
        }
        TestRecord::RunAgain => {
            " The editor has changed since this run, so it cannot complete Test: before leaving Test, ask them to click Run on the code now on screen."
        }
        TestRecord::Settled => "",
    };
    let analysed = phases_evidenced(state, &[FrameworkPhase::Optimizations]);
    if all_passed && analysed && since_previous != SincePrevious::Rewritten {
        // A result can land after the candidate has typed past the code it ran.
        // Then this run does not describe the editor, and another run of what
        // is there now may be exactly what the change needs.
        let matches_editor = if state.interview_loop == InterviewLoop::CodingOnly {
            crate::agent::tested_code_is_exact(state)
        } else {
            crate::agent::tested_code_is_current(state)
        };
        let (rerun, not_again) = if !matches_editor {
            (
                " The editor has changed since this run, so it may not describe the code now on screen; ask for a run of that code only if the change could affect the result.",
                "complexity or edge cases",
            )
        } else if since_previous == SincePrevious::Unchanged {
            (
                " The code is unchanged since their previous run.",
                "complexity, edge cases or another run",
            )
        } else {
            ("", "complexity, edge cases or another run")
        };
        let next = if state.interview_loop == InterviewLoop::CodingOnly {
            coding_only_continuation(state)
        } else {
            "If the coding discussion is complete, wrap it up under the round plan; do not start a behavioral question in this same reply.".to_string()
        };
        return format!(
            "[SYSTEM EVENT] The candidate just ran the built-in test cases and every one passed:\n{summary_text}\n{code}Reported, not proof.{record}{rerun} Complexity and edge cases are already covered: acknowledge the result in one short sentence and do not ask for {not_again}. {next}"
        );
    }
    if all_passed {
        // The rewritten case keeps the earlier analysis in view: what is asked
        // is whether it still holds, not the whole step again.
        let before = if analysed {
            " The code changed substantially since they gave the complexity, so it may no longer describe this code: unless they already answered for the new code, ask only whether it still holds."
        } else if since_previous == SincePrevious::Unchanged {
            " The code is unchanged since their previous run, so this repeats a result already discussed."
        } else {
            ""
        };
        return format!(
            "[SYSTEM EVENT] The candidate just ran the built-in test cases and every one passed:\n{summary_text}\n{code}Reported, not proof.{record} Acknowledge it briefly. {RECORD_UNRECORDED}{before} Otherwise move to Optimizations with ONE short question asking only for what they have not covered: an adversarial edge case plus either confirmed time/space complexity or one useful optimization/refactor. Accept an already-optimal answer when justified. Two sentences maximum; do not start a behavioral question in this same reply."
        );
    }

    format!(
        "[SYSTEM EVENT] The candidate just ran the built-in test cases and some failed:\n{summary_text}\n{code}Reported, not proof.{record} Then go back to diagnosis: in one or two short sentences, ask the candidate to choose one failing case, state its expected result and what their code produced, then name the assumption they will inspect. Do not state the commonality, bug, location, or fix, and do not name a data structure, algorithm, or invariant. Reference a failing input only if needed and never read raw code or values symbol by symbol."
    )
}

pub fn test_setup_error_reaction(summary_text: &str, excerpt: Option<&str>) -> String {
    let code = reaction_code(excerpt);
    format!(
        "[SYSTEM EVENT] The candidate tried to run the built-in test cases, but the runner reported a setup error:\n{summary_text}\n{code}Reported, not proof. Return from Test to Coding: in one or two short sentences, ask the candidate to read the first setup error, say whether it prevents loading the tests, compilation, or execution, then name the one assumption they will verify before running again. Do not identify the error's cause, location, or fix, and do not provide code, commands, a data structure, algorithm, or invariant. Never read raw code or error text symbol by symbol."
    )
}

pub fn test_runner_unavailable_reaction(summary_text: &str, excerpt: Option<&str>) -> String {
    let code = reaction_code(excerpt);
    format!(
        "[SYSTEM EVENT] The candidate tried to run the built-in test cases, but the platform could not provide them:\n{summary_text}\n{code}This is not the candidate's error. In one or two short sentences, say the runner is unavailable, that they may click Run once more later, and ask them to trace their code by hand on one ordinary case and one boundary case, stating the expected result of each. While the runner stays unavailable in this language, the platform records Test from that trace of the written code. Do not diagnose the platform, and never read raw code or error text symbol by symbol."
    )
}

/// An outage once no trace is wanted: Test is already recorded, or a run still
/// covers the code on screen. It used to reach the setup-error reaction, which
/// asks the candidate to find the stage that failed and an assumption to
/// verify, about a failure no edit of theirs can cause or fix.
pub fn test_runner_outage_reaction(summary_text: &str, excerpt: Option<&str>) -> String {
    let code = reaction_code(excerpt);
    format!(
        "[SYSTEM EVENT] The candidate tried to run the built-in test cases, but the platform could not run them:\n{summary_text}\n{code}This is not the candidate's error and says nothing about their code. In one short sentence, say the runner is unavailable and that they may click Run once more later, then continue from the conversation; do not choose the next step from this run. Do not ask them to diagnose the failure or to walk through their code instead, do not diagnose the platform, and never read raw code or error text symbol by symbol."
    )
}
