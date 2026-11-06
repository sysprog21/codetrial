//! The prompts the room loop sends on its own initiative: silence nudges,
//! proactive review, time warnings, round start and skip, the refusals to end
//! early, and the wrap-up.

use super::instructions::DECLINED_PROBE;
use super::progress::{
    candidate_code_presence_given, coding_only_continuation, coding_only_ending, coding_progress,
    coding_progress_without_presence, record_unrecorded, verified_outcome,
};
use crate::agent::{
    FrameworkPhase, InterviewLoop, InterviewMode, RuntimeState, SILENCE_THRESHOLD_S, TestSource,
    coding_continues_past_gate, phases_evidenced,
};

/// Its conditions come before the invitation: a model follows the first
/// imperative it reads, and an invitation read first would press a declined
/// probe or ask after an answer the round never requested.
pub fn behavioral_silence_nudge() -> String {
    format!(
        "[SYSTEM EVENT] The behavioral round has been quiet for at least {SILENCE_THRESHOLD_S:.0} seconds. Silence alone does not mean the answer is complete or that there is nothing further to discuss, so do not use `end_interview` because of it. Use the conversation to decide which case applies, taking the first that does. If the round's one STAR question has not been asked yet: {} Otherwise, if {DECLINED_PROBE}, or the answer is already complete, invite nothing further on that probe: leave its unsupported parts unassessed, retain any evidence already given, and use `end_interview` under its normal completion rules. Otherwise, offer one brief, neutral invitation to continue the current answer, without adding a question or requesting a missing STAR part. Do not suggest an example, return to coding, or repeat or replace the round's question.",
        round_question()
    )
}

/// How a watch prompt points at the code: at the excerpt it carries when the
/// editor changed since the model last saw it, and otherwise at the code it
/// already holds. Pointing at `read_editor` there was a tool round trip for a
/// buffer the model had been shown.
fn code_access(excerpt: Option<&str>) -> String {
    match excerpt {
        Some(excerpt) => format!(
            "Their code, numbered; `read_editor` shows anything it leaves out.\n{excerpt}\n"
        ),
        None => "The editor is unchanged since you last saw it.\n".to_string(),
    }
}

/// The evidence a watch prompt carries, which is nothing at all when none of
/// its lines changed since the last one: the model already holds them.
fn evidence_section(evidence: &str) -> String {
    if evidence.is_empty() {
        "\n".to_string()
    } else {
        format!(" Deterministic session evidence:\n{evidence}\n")
    }
}

/// The coding round's silence nudge.
///
/// Without the test record this told a candidate whose code had already passed
/// to go and test it, and a candidate who had finished complexity and edge
/// cases in the silence that followed was sent back to the Test step.
pub fn silence_nudge(state: &RuntimeState, evidence: &str, excerpt: Option<&str>) -> String {
    let setup_error = state
        .last_test_run
        .as_ref()
        .is_some_and(crate::agent::evidence::run_failed_to_start);
    let with_code = match coding_progress_without_presence(state) {
        Some(progress) if coding_continues_past_gate(state) => {
            progress
        }
        Some(progress) if crate::agent::coding_round_complete(state) => format!(
            "{progress} Ask whether they have anything to add, or wrap up the coding discussion under the round plan."
        ),
        Some(progress) => {
            format!("{progress} Ask about the next step they have not covered.")
        }
        None if setup_error => "The latest attempt hit a runner setup error, not completed tests: ask what they will verify before a retry, which needs no code change, without supplying the cause or fix.".to_string(),
        None => "Ask them to narrate or test their code.".to_string(),
    };
    let written = crate::agent::code_written(state);
    let with_code = match candidate_code_presence_given(state, written) {
        Some(presence) => format!("{with_code} {presence}"),
        None => with_code,
    };

    // With all three planning steps recorded there is nothing left to ask for,
    // and the question that moves them on is the code.
    let unwritten = if phases_evidenced(
        state,
        &[
            FrameworkPhase::Repeat,
            FrameworkPhase::Example,
            FrameworkPhase::Algorithm,
        ],
    ) {
        "ask them to begin writing their code in the editor."
    } else {
        "ask for whichever of their understanding, example, or planned algorithm they have not explained."
    };

    // Repeat, Example and Algorithm recorded with no code yet means the stall
    // is in Coding, not in an earlier step, as far as accumulated evidence can
    // say — `phases_evidenced` only sees that each phase has evidence somewhere
    // in the session, not that the live conversation has left Example for good.
    // A candidate can cover all three and then return to Example, with a
    // concrete concern, before touching the editor, so this stays a scoped
    // conditional (`candidate_led_scoped_question`) rather than an outright
    // substitution for `unwritten`: the model, which can see whether the stall
    // is actually in Algorithm or Coding, decides, and a stall still in Example
    // gets exactly what it would without candidate-led enabled. The trailing
    // append below is skipped whenever a branch already carries that scoped
    // clause itself, which is every `!written` branch, to avoid doubling it up.
    let candidate_led_certain = state.candidate_led_enabled
        && phases_evidenced(
            state,
            &[
                FrameworkPhase::Repeat,
                FrameworkPhase::Example,
                FrameworkPhase::Algorithm,
            ],
        );

    // The server knows whether the editor holds edits, so the nudge says which
    // rather than offering both branches: asked to choose, the model read the
    // editor first, a tool round in front of the question. Only an editor
    // holding just the starter is certainly unwritten; edits too small to count
    // or too long to compare leave the choice to the editor.
    let branch = if written {
        format!(" {with_code}")
    } else if crate::agent::code_untouched(state) {
        if candidate_led_certain {
            format!(
                " They have not written code yet: {}{unwritten}",
                candidate_led_scoped_question()
            )
        } else {
            format!(" They have not written code yet: {unwritten}")
        }
    } else if candidate_led_certain {
        format!(
            " {}if the editor holds none of their code, {unwritten} Otherwise: {with_code}",
            candidate_led_scoped_question()
        )
    } else {
        format!(" If the editor holds none of their code, {unwritten} Otherwise: {with_code}")
    };
    format!(
        "[SYSTEM EVENT] Silent and not typing for over {SILENCE_THRESHOLD_S:.0} seconds.{}{}Flow 2: ONE short question about their current decision.{branch} Do not restart them, restate the problem, supply an example, suggest an approach or reveal a bug.{}{}",
        evidence_section(evidence),
        code_access(excerpt),
        behavioral_ban(state),
        if candidate_led_certain && !written {
            ""
        } else {
            candidate_led_nudge(state.candidate_led_enabled)
        },
    )
}

/// The scoped clause `candidate_led_certain` branches lead with instead of
/// asking `CANDIDATE_LED_OPEN_QUESTION` outright: see the comment on
/// `candidate_led_certain` above for why evidence alone cannot earn the
/// unconditional form. Ends with a lowercase-ready "Otherwise, " so a caller
/// appends its own fallback text directly.
fn candidate_led_scoped_question() -> String {
    format!(
        "if this stall is in the Algorithm or Coding step, {CANDIDATE_LED_OPEN_QUESTION} Otherwise, "
    )
}

/// The open question asked, scoped by `candidate_led_scoped_question` or by
/// `candidate_led_nudge`, in place of naming the next step.
const CANDIDATE_LED_OPEN_QUESTION: &str = "ask what they are thinking of doing next \
without naming or steering toward one yourself, and wait for them to state it.";

/// Said by every silence nudge, editor and board alike, when the candidate
/// opted to lead the Algorithm and Coding steps: the fresh instruction this
/// stall is answered under, where it has to hold, not only the system prompt
/// set once at the start of the session. Appended, rather than scoped inline
/// like `candidate_led_scoped_question`, for the branches in `silence_nudge`
/// that never name a next step to replace (the `written` branch, where code
/// can exist in Test or Optimizations too) — there the model needs the scope
/// stated once, not woven into text that already stands on its own.
fn candidate_led_nudge(enabled: bool) -> &'static str {
    if enabled {
        " If this stall is in the Algorithm or Coding step, ask what they are \
thinking of doing next without naming or steering toward one yourself, and \
wait for them to state it; outside those two steps, ask as above."
    } else {
        ""
    }
}

/// The silence nudge for a board, which carries no snapshot of its own.
///
/// The editor's version quotes the code into the prompt; the board cannot be
/// quoted, and the image the interviewer already has is the one it would have
/// sent. What is left to say is how much is on it, which is what decides
/// whether the question should be about the drawing or about the problem.
pub fn board_silence_nudge(evidence: &str, strokes: u32, candidate_led_enabled: bool) -> String {
    let board = if strokes == 0 {
        "The board is still empty.".to_string()
    } else {
        format!("The board holds {strokes} strokes, and you have the latest image of it.")
    };
    format!(
        "[SYSTEM EVENT] Silent and not drawing for over {SILENCE_THRESHOLD_S:.0} seconds.{}{board}\nFlow 2: ONE short question about their current decision. If the board is empty, ask for whichever of their understanding, example, or planned algorithm they have not explained; if there is a drawing, ask them to narrate or trace it, and refer to a part of it only after looking at the image. Do not restart them, restate the problem, supply an example, suggest an approach or reveal a bug.{BEHAVIORAL_BAN}{}",
        evidence_section(evidence),
        candidate_led_nudge(candidate_led_enabled),
    )
}

/// Fired by an editor change, which is the one case where a test already run
/// may no longer describe the code: the progress clause states whether it does.
pub fn proactive_review(state: &RuntimeState, evidence: &str, excerpt: Option<&str>) -> String {
    format!(
        "[SYSTEM EVENT] A code change settled.{}{}{}",
        evidence_section(evidence),
        code_access(excerpt),
        proactive_review_policy(state),
    )
}

/// A coding-only session's system instruction already rules the behavioral
/// round out, so only a two-round session needs it said here.
fn behavioral_ban(state: &RuntimeState) -> &'static str {
    if state.interview_loop == crate::agent::InterviewLoop::CodingOnly {
        ""
    } else {
        BEHAVIORAL_BAN
    }
}

const BEHAVIORAL_BAN: &str =
    " Never ask, repeat, or return to a behavioral or experience question here.";

pub(super) fn proactive_review_policy(state: &RuntimeState) -> String {
    let progress = coding_progress(state).map_or(String::new(), |progress| format!(" {progress}"));
    format!(
        "Flow 1: speak only for a real bug, a finished block or a missed transition, with ONE question, such as the reasoning behind a change, the complexity, or a predicted test; otherwise 'mm-hm' or nothing, and never restart them.{} Naming or ruling out an algorithm, data structure, invariant or bug location is a hint: `log_hint` with `requested` false.{progress}",
        behavioral_ban(state),
    )
}

pub(crate) fn tool_editor_review(state: &RuntimeState) -> String {
    format!(
        "Review the current code supplied in this response in the reply already owed to the candidate. Answer their latest question first; do not add another question if one is already pending. {}",
        proactive_review_policy(state),
    )
}

/// The coding round's five-minute warning. Its convergence order names only
/// the steps still open, so a candidate who has tested, stated the complexity,
/// or finished outright is not sent back to them. Where Test is open and only a
/// hand trace can complete it, asking for a Run would spend the candidate's
/// last minutes on a runner that cannot answer.
pub fn time_warning(state: &RuntimeState) -> String {
    let order = if coding_continues_past_gate(state) {
        match verified_outcome(state) {
            Some(true) => "confirm any final change, then discuss only what they have not covered yet",
            Some(false) => "prioritize the unresolved failures and verify the fix, then discuss only what remains uncovered",
            None => "verify the code now on screen, then discuss only what remains uncovered",
        }
        .to_string()
    } else if crate::agent::coding_round_complete(state) {
        "confirm any final change, then add anything about the solution they have not covered yet"
            .to_string()
    } else {
        let mut steps = vec![if state.interview_mode.is_whiteboard() {
            "finish tracing one example through the drawing"
        } else {
            "finish a testable core"
        }];

        // A run of the code on screen already completes Test once recorded, so
        // asking for another spends the last minutes repeating it.
        let source = crate::agent::test_source(state);
        if !phases_evidenced(state, &[FrameworkPhase::Test]) && source != TestSource::Run {
            steps.push(match source {
                TestSource::Trace if state.code_execution_disabled => {
                    "trace the highest-value cases by hand, since code execution is disabled for this interview"
                }
                TestSource::Trace => {
                    "trace the highest-value cases by hand, since the runner cannot provide tests for this language"
                }
                TestSource::Board => {
                    "name the highest-value cases that would break the drawing"
                }
                TestSource::Run | TestSource::Neither => {
                    "click Run on the highest-value tests"
                }
            });
        }

        // Conditional even while unrecorded: the candidate may have said it
        // already, and the order is read as a command.
        if !phases_evidenced(state, &[FrameworkPhase::Optimizations]) {
            steps.push("state time and space complexity unless they already have");
        }
        steps.join(", then ")
    };
    let progress = coding_progress(state).map_or(String::new(), |progress| format!(" {progress}"));
    format!(
        "[SYSTEM EVENT] The interview timer has reached the five-minute warning. Briefly and naturally warn the candidate and give this convergence order: {order}.{progress} Two short sentences maximum. Do not start a behavioral question now."
    )
}

/// The five-minute warning once the behavioral round owns the clock. The coding
/// round's convergence order above would send the candidate back to the editor.
pub fn behavioral_time_warning() -> String {
    format!(
        "[SYSTEM EVENT] The behavioral round has reached the five-minute warning. Do not return to coding or ask a new question. Let the candidate finish the current answer, ask at most the one permitted neutral missing-STAR follow-up, only if it has not already been used and not when {DECLINED_PROBE}, then close naturally. Never reopen an abandoned behavioral probe."
    )
}

/// The round's one question, for every briefing that may ask it: the
/// transition that opens the round, and a recovery or silence nudge that finds
/// it unasked. A probe declined before the round must not come back as that
/// question; one declined inside it already was the question, which is why a
/// recovery states where the round begins.
pub(super) fn round_question() -> String {
    format!(
        "Ask exactly one concise question under the private STAR, profile, and document-grounding policies. If {DECLINED_PROBE} for an earlier behavioral question, that probe stays closed: do not repeat or rephrase it, and choose a clearly different theme for this round's one question."
    )
}

/// Where the behavioral round begins in the transcript. An interviewer turn
/// still in flight when the round opened keeps rewriting its own earlier line,
/// so a change to that line since the transition moves the start back to it.
/// Candidate lines interleaved before the transition come with it. A declined
/// behavioral question there counts as the round's question, so the round then
/// goes without one and its STAR parts stay unassessed: accepted over asking
/// for another story right after a refusal.
pub(crate) fn behavioral_round_start(state: &RuntimeState) -> usize {
    state
        .behavioral_round_prior_turn
        .as_ref()
        .filter(|(index, before)| state.transcript.get(*index) != Some(before))
        .map_or(state.behavioral_round_transcript_start, |(index, _)| *index)
}

/// The line the report transcript carries where the platform opened the
/// behavioral round. It has no speaker, so a candidate who says the same words
/// is still a `Candidate:` line.
pub(crate) const BEHAVIORAL_ROUND_MARK: &str = "(the platform opened the behavioral round here)";

/// The transcript the report reads, with `BEHAVIORAL_ROUND_MARK` where the
/// round began.
///
/// The transcript is speech only, so without the mark a behavioral answer the
/// interviewer asked for out of turn during coding reads the same as the one
/// the round asked for, and the report could score it. The live tool refused
/// STAR evidence for the first, which leaves the transcript the only place it
/// survives. Marked where `behavioral_round_start` puts the round, so an
/// interviewer turn still in flight at the transition falls inside it.
pub(crate) fn report_transcript_lines(state: &RuntimeState) -> Vec<String> {
    let mut lines = state.delivery_transcript().into_owned();
    if state.behavioral_round_started {
        let start = behavioral_round_start(state).min(lines.len());
        lines.insert(start, BEHAVIORAL_ROUND_MARK.to_string());
    }
    lines
}

/// The reserved behavioral round, opened because the coding gate passed.
pub fn round_started() -> String {
    format!(
        "[SYSTEM EVENT] The trusted coding completion gate passed: Test and Optimizations both have candidate evidence. The coding round is closed. Begin the reserved behavioral round now. {} Use prior candidate answers only to deepen the follow-up; do not repeat them and do not return to coding.",
        round_question()
    )
}

/// The reserved behavioral round, refused because the coding gate did not pass.
pub fn round_skipped(mode: InterviewMode) -> String {
    format!(
        "[SYSTEM EVENT] The behavioral reserve began, but the trusted coding completion gate did not pass because Test or Optimizations evidence is absent. Do not start STAR. {} Otherwise keep the candidate focused on a testable solution, highest-value tests, and justified complexity until the session ends; missing STAR phases will be marked skipped.",
        record_unrecorded(mode)
    )
}

/// Why `end_interview` is refused: coding-only time belongs to the candidate,
/// and an unfinished round needs only the steps still missing. With Test
/// recorded, inviting a Run sends a
/// finished candidate back to Test; without it, the way to Test is the one the
/// gate is in, since a Run button is no help while the runner is missing.
pub(crate) fn end_interview_refusal(state: &RuntimeState) -> String {
    if coding_continues_past_gate(state) {
        return coding_only_continuation(state);
    }
    let refusal = unfinished_coding_refusal(state);
    if state.interview_loop == InterviewLoop::CodingOnly {
        format!("{} {refusal}", coding_only_ending(state))
    } else {
        refusal
    }
}

/// In a coding-only session missing evidence is not what stands between the
/// interviewer and the end, so the refusal must not read as though recording
/// it would make `end_interview` succeed.
fn unfinished_coding_refusal(state: &RuntimeState) -> String {
    let unfinished = if state.interview_loop == InterviewLoop::CodingOnly {
        ""
    } else {
        ", so the interview is not finished"
    };
    let records = if crate::agent::evidence_tool_declared(state.interview_mode) {
        "record evidence when the candidate earns it"
    } else {
        "the platform records evidence when the candidate earns it"
    };
    if phases_evidenced(state, &[FrameworkPhase::Test]) {
        return format!(
            "The coding round has no Optimizations evidence yet{unfinished}. {} Otherwise ask only for what they have not covered, and {records}.",
            record_unrecorded(state.interview_mode)
        );
    }
    let missing = if phases_evidenced(state, &[FrameworkPhase::Optimizations]) {
        "Test"
    } else {
        "Test and Optimizations"
    };
    let way_to_test = match crate::agent::test_source(state) {
        TestSource::Trace if state.code_execution_disabled => {
            "Code execution is disabled for this interview, so ask the candidate to trace their written code by hand, which the platform records as Test"
        }
        TestSource::Trace => {
            "The runner cannot provide tests for this language, so ask the candidate to trace their code by hand, which the platform records as Test"
        }
        TestSource::Board => {
            "Nothing runs at a whiteboard, so ask the candidate to name the cases that would break their drawing and record Test from what they say or draw"
        }
        TestSource::Run | TestSource::Neither => {
            "If the candidate has not run the code now in the editor, invite them to click Run and wait for the results"
        }
    };
    format!(
        "The coding round has no {missing} evidence yet{unfinished}. {way_to_test}; otherwise continue, and {records}."
    )
}

/// The platform has already closed the STAR steps of a round that never
/// opened. Inside one that did, a step without evidence is either cut off by
/// the clock or part of a probe the candidate declined, and only the
/// conversation tells them apart, so an interviewer holding the evidence tool
/// records those skips (`interviewer_skips`); without it the platform does,
/// from the phase judge's reading.
pub fn wrap_up(reason: &str, interviewer_skips: bool) -> String {
    let why = match reason {
        "time_up" => "the timer has run out",

        // The interviewer's own call, so the closing has to read as a decision
        // rather than as an interruption: nothing ran out, the interview
        // finished.
        "interview_complete" => "you judged the interview complete",
        _ => "the candidate chose to end the session",
    };
    let skips = if interviewer_skips {
        format!(
            " For each STAR phase not already evidenced, silently call `record_framework_evidence` once with source `session_timing`, kind `skipped`, confidence 100, and a short summary that the session ended before assessment, except where {DECLINED_PROBE}: leave that probe's unsupported parts unassessed and retain any evidence already given. Do not speak those calls."
        )
    } else {
        String::new()
    };
    format!(
        "[SYSTEM EVENT] The interview is over because {why}. Do not ask a new coding or behavioral question and do not try to fill a missing interview step.{skips} In at most two short sentences, thank the candidate warmly and tell them their written performance report is being prepared and will appear on screen in a moment. Do not speak scores, the checklist, or the hiring decision."
    )
}
