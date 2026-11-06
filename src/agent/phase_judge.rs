//! The phase judge: when a phase is due for judgment, the window of interview
//! it reads, and how its answer is checked against what was said before it is
//! recorded.

// This module reads the hub's imports and types through `use super::*`.
#[cfg_attr(
    not(test),
    expect(
        clippy::wildcard_imports,
        reason = "the module reads its hub through super::*"
    )
)]
use super::*;

/// The REACTO steps the phase judge records. Test is the platform's from the
/// run itself; these are the ones only understanding can tick.
const JUDGED_PHASES: [FrameworkPhase; 5] = [
    FrameworkPhase::Repeat,
    FrameworkPhase::Example,
    FrameworkPhase::Algorithm,
    FrameworkPhase::Coding,
    FrameworkPhase::Optimizations,
];

/// The behavioral round's steps, which the judge records once the round has
/// opened, from what the candidate said inside it.
const JUDGED_STAR: [FrameworkPhase; 4] = [
    FrameworkPhase::Situation,
    FrameworkPhase::Task,
    FrameworkPhase::Action,
    FrameworkPhase::Result,
];

/// Every step a judgment may name, for reading one back.
const JUDGEABLE: [FrameworkPhase; 10] = [
    FrameworkPhase::Repeat,
    FrameworkPhase::Example,
    FrameworkPhase::Algorithm,
    FrameworkPhase::Coding,
    FrameworkPhase::Test,
    FrameworkPhase::Optimizations,
    FrameworkPhase::Situation,
    FrameworkPhase::Task,
    FrameworkPhase::Action,
    FrameworkPhase::Result,
];

/// Transcript the judge reads, the latest stretch within the bound. A tail
/// rather than only what is new, so a step an earlier call missed can still be
/// quoted while it is in reach.
const PHASE_JUDGE_WINDOW_BYTES: usize = 6 * 1024;
const PHASE_JUDGE_CODE_BYTES: usize = 6 * 1024;

/// The fewest words a quote may have. A restatement, a worked case or an
/// approach takes a sentence, so six; complexity is short ("it is O of n
/// time") and Coding quotes code, so four. Either way a quote cannot be a
/// stray phrase that happens to appear in a candidate line.
const MIN_SPOKEN_QUOTE_WORDS: usize = 6;
const MIN_QUOTE_WORDS: usize = 4;
/// What the row keeps of the judge's summary and of the quote it was checked
/// against. The quote goes into the row, so the report reads the words that
/// were verified rather than only the judge's account of them.
const MAX_PHASE_JUDGE_SUMMARY_CHARS: usize = 120;
const MAX_PHASE_JUDGE_QUOTE_CHARS: usize = 110;

/// The steps the judge may still record, in checklist spelling: the STAR
/// steps once the behavioral round has opened, the coding ones before it, and
/// none about code before there is code.
///
/// Not closed by the end of the interview: the end waits for a judgment of
/// the candidate's last answer, and the report is written after it.
///
/// Once Coding is recorded, Repeat, Example and Algorithm are asked about
/// only beside a step about the code. A candidate who skipped one would
/// otherwise keep a call due after every turn for the rest of the session,
/// spending the budget before Optimizations. An interviewer holding the
/// evidence tool records one explained late itself, so for it they close.
///
/// Test is the run's, so it is open only while nothing can run the code: a
/// hand trace is then the only testing, and without the evidence tool the
/// judge is the only one who can record it. The STAR steps likewise, which an
/// interviewer holding the tool records.
pub fn phase_judge_open(state: &RuntimeState) -> Vec<&'static str> {
    let tool = evidence_tool_declared(state.interview_mode);
    if state.behavioral_round_started {
        return JUDGED_STAR
            .into_iter()
            .filter(|phase| !tool && !phases_evidenced(state, &[*phase]))
            .map(phase_id)
            .collect();
    }
    let coded = phases_evidenced(state, &[FrameworkPhase::Coding]);
    let traced = !tool && test_source(state) == TestSource::Trace;
    let unrecorded = JUDGED_PHASES
        .into_iter()
        .chain(traced.then_some(FrameworkPhase::Test))
        .filter(|phase| !phases_evidenced(state, &[*phase]))
        .filter(|phase| phase.needs_code() || !coded || !tool)
        .collect::<Vec<_>>();

    // Asked last and once: whether there is written work compares the editor
    // with the starter, and nothing about code may still be open. Coding is
    // grounded in typed code, so a whiteboard, where the work is a drawing,
    // leaves it to the interviewer; Optimizations is grounded in what they say
    // about the work, drawn or typed.
    let has_work = unrecorded.iter().any(|phase| phase.needs_code()) && written_work(state);
    let typed = !state.interview_mode.is_whiteboard();
    let open = unrecorded
        .into_iter()
        .filter(|phase| match phase {
            FrameworkPhase::Coding => has_work && typed,
            phase if phase.needs_code() => has_work,
            _ => true,
        })
        .collect::<Vec<_>>();
    if coded && !open.iter().any(|phase| phase.needs_code()) {
        return Vec::new();
    }
    open.into_iter().map(phase_id).collect()
}

/// The candidate's latest turn, which the recognizer keeps rewriting in place
/// while they speak, so a judge that read it halfway knows to read it again.
/// Only from where the judge reads, so a coding-round turn is not new to a
/// behavioral round that has heard nothing yet.
fn last_candidate_line(state: &RuntimeState) -> &str {
    last_speaker_line(&state.transcript[judged_from(state)..], CANDIDATE_SPEAKER)
        .map_or("", |(_, line)| line)
}

/// Why a judgment is due, if one is: for something the candidate said, or for
/// the editor alone. Typing changes the editor on every pause, so a call for it
/// alone is paced and capped apart from the others; see `claim_phase_judge`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhaseJudgeDue {
    No,
    Spoken,
    EditorOnly,
}

/// Whether there is anything new for the judge: a candidate turn it has not
/// been shown, the latest one grown since, or for an open Coding step, an
/// editor it has not read. The cursors are compared before the open steps are
/// worked out, so an idle tick with nothing new costs two comparisons. A
/// behavioral round that turns out to open earlier than the judge read it
/// from is new as well: the lines it gained were never shown.
pub fn phase_judge_need(state: &RuntimeState) -> PhaseJudgeDue {
    let start = judged_from(state);
    let from = state
        .judged_transcript_lines
        .max(start)
        .min(state.transcript.len());
    let spoke = candidate_lines(&state.transcript[from..]) > 0
        || last_candidate_line(state) != state.judged_last_turn
        || start < state.judged_round_from;
    let typed = state.code != state.judged_code || state.language != state.judged_language;
    if !spoke && !typed {
        return PhaseJudgeDue::No;
    }
    let open = phase_judge_open(state);
    if spoke && !open.is_empty() {
        PhaseJudgeDue::Spoken
    } else if typed
        && (open.contains(&"coding")
            || (!state.behavioral_round_started
                && !open.is_empty()
                && typed_comments(&state.language, &state.code)
                    != typed_comments(&state.judged_language, &state.judged_code)))
    {
        PhaseJudgeDue::EditorOnly
    } else {
        PhaseJudgeDue::No
    }
}

/// Whether a judgment is due for any reason.
pub fn phase_judge_due(state: &RuntimeState) -> bool {
    phase_judge_need(state) != PhaseJudgeDue::No
}

/// The judge's prompt, with its cursors moved past what it shows.
///
/// Moved when the call goes out, as the interim window is: a call that fails
/// costs nothing, because the next one reads the same tail.
pub fn take_phase_judge_window(state: &mut RuntimeState, problem: &Problem) -> String {
    // Only the tail is marked, since only the tail is sent. A behavioral round
    // is read from where it opened: an answer given before it was asked for
    // counts for nothing, as the report reads it.
    let delivery = state.delivery_transcript();
    let lines = &delivery[judged_from(state)..];
    let tail = &lines[tail_start(lines, PHASE_JUDGE_WINDOW_BYTES)..];
    let window = transcript_tail(&mark_unrecognized_turns(tail), PHASE_JUDGE_WINDOW_BYTES);
    state.judged_for = phase_judge_need(state);
    state.judged_transcript_lines = state.transcript.len();
    state.judged_round_from = judged_from(state);
    state.judged_last_turn = last_candidate_line(state).to_string();
    state.judged_code = state.code.clone();
    state.judged_language = state.language.clone();

    // Written code goes with every coding-round step: a hand trace is about it,
    // and an explanation the candidate typed as a comment, as they are asked to
    // when their speech was not recognized, is quoted from it.
    let open = phase_judge_open(state);
    let code = if open
        .iter()
        .any(|id| matches!(*id, "coding" | "optimizations"))
        || (!state.behavioral_round_started && code_written(state))
    {
        code_head(&state.code, PHASE_JUDGE_CODE_BYTES)
    } else {
        String::new()
    };
    let prompt = phase_judge_prompt(&PhaseJudgeInput {
        problem,
        open: &open,
        transcript_window: &window,
        code: &code,
        language: &state.language,
        interview_mode: state.interview_mode,
    });
    state.evidence_ledger.record_model_input(
        ModelInputKind::Interim,
        &format!("{}\n\n{prompt}", phase_judge_system_instruction()),
    );
    prompt
}

/// Lowercase words, punctuation dropped: a quote is checked for the words the
/// candidate said, not for how the recognizer punctuated them.
fn quote_words(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c.is_alphanumeric() {
                c.to_lowercase().next().unwrap_or(c)
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// The comments in `code`, read by the lexer that leaves them out of a
/// comparison, so the two never disagree on what a comment is: a marker inside
/// a string is code, a comment trailing a line of code counts, and a C line
/// beginning with `*` is a dereference outside a block. Read apart from the
/// code, so a step is quoted from an explanation, not from the code that is
/// Coding's.
pub(super) fn typed_comments(language: &str, code: &str) -> String {
    split_comments(language, code).1
}

/// One speaker's turn, as `speaker_turns` reads the transcript.
pub(super) struct Turn {
    pub(super) speaker: &'static str,
    /// Its normalized words, padded with a space at each end so a quote is
    /// found as whole words with one `contains`.
    pub(super) words: String,
}

/// The first transcript line the judge reads and may quote: the opening of
/// the behavioral round once it has one, else the whole conversation.
fn judged_from(state: &RuntimeState) -> usize {
    if state.behavioral_round_started {
        behavioral_round_start(state).min(state.transcript.len())
    } else {
        0
    }
}

/// The transcript as turns: adjacent lines of one speaker joined, since the
/// recognizer can split one answer into two lines with nothing between them.
/// An unrecognized line ends a turn without joining one, as it carries no
/// words. Only what the judge may quote, from `judged_from`.
pub(super) fn speaker_turns(state: &RuntimeState) -> Vec<Turn> {
    let mut turns: Vec<Turn> = Vec::new();
    let mut joinable = false;
    for line in &state.transcript[judged_from(state)..] {
        let spoken = [CANDIDATE_SPEAKER, INTERVIEWER_SPEAKER]
            .into_iter()
            .find_map(|speaker| speaker_speech(line, speaker).map(|text| (speaker, text)));
        let Some((speaker, text)) = spoken.filter(|_| !is_unrecognized_turn(line)) else {
            joinable = false;
            continue;
        };
        let words = quote_words(text);
        match turns.last_mut() {
            Some(turn) if joinable && turn.speaker == speaker => {
                turn.words.push_str(&words);
                turn.words.push(' ');
            }
            _ => turns.push(Turn {
                speaker,
                words: format!(" {words} "),
            }),
        }
        joinable = true;
    }
    turns
}

/// Where `quote` is the candidate's own, if it is: words from one recognized
/// candidate turn, or what they typed into the editor, its code for Coding and
/// a comment for another coding-round step.
///
/// Proof of who said it, not that the step was done; that judgment stays with
/// the model and its prompt. What can be checked mechanically is checked: the
/// starter is not the candidate's code, and an approach or an analysis the
/// interviewer said before the candidate did is the candidate agreeing, not
/// explaining. The interviewer saying it back afterwards, which is how an
/// interviewer confirms they followed, leaves the candidate's words theirs.
fn quote_is_grounded(
    state: &RuntimeState,
    turns: &[Turn],
    phase: FrameworkPhase,
    quote: &str,
) -> Option<EvidenceSource> {
    let words = quote_words(quote);

    // A spoken trace is a sentence like any other answer, though Test is about
    // code.
    let least = if matches!(
        phase,
        FrameworkPhase::Coding | FrameworkPhase::Optimizations
    ) {
        MIN_QUOTE_WORDS
    } else {
        MIN_SPOKEN_QUOTE_WORDS
    };
    if words.split(' ').count() < least {
        return None;
    }
    let needle = format!(" {words} ");
    let typed = |text: &str| format!(" {} ", quote_words(text)).contains(&needle);
    let editor = Some(EvidenceSource::EditorSnapshot);
    if phase == FrameworkPhase::Coding {
        return editor.filter(|_| typed(&state.code) && !typed(starter(state, &state.language)));
    }

    // A candidate whose speech was not recognized is asked to type their
    // explanation as a comment, and at an editor without the evidence tool
    // nobody else can record it from there. A hand trace stays spoken: Test
    // without a run is recorded as the candidate's speech, which is how the
    // report tells tracing from testing.
    let commented = |code: &str| typed(&typed_comments(&state.language, code));
    if !phase.is_star()
        && phase != FrameworkPhase::Test
        && !state.interview_mode.is_whiteboard()
        && commented(&state.code)
        && !commented(starter(state, &state.language))
    {
        return editor;
    }
    let mut said = turns.iter().filter(|turn| turn.words.contains(&needle));
    let first = said.clone().next()?;
    if !said.any(|turn| turn.speaker == CANDIDATE_SPEAKER) {
        return None;
    }

    // A trace the interviewer walked first, or a story they told back before
    // the candidate did, is the candidate agreeing too.
    let echo = (phase.is_star()
        || matches!(
            phase,
            FrameworkPhase::Algorithm | FrameworkPhase::Optimizations | FrameworkPhase::Test
        ))
        && first.speaker == INTERVIEWER_SPEAKER;
    (!echo).then_some(EvidenceSource::CandidateSpeech)
}

/// Records what the judge found, through the same gate the interviewer's calls
/// pass, and returns the rows it added.
///
/// The judge is a model too, so nothing it says is taken on its word: the step
/// has to be one still open when the answer lands, and the quote has to be the
/// candidate's own words, or for Coding the editor's. A step the interviewer
/// recorded while the call was out is no longer open and is left alone, and a
/// judgment about code the candidate has since rewritten is dropped, since it
/// would anchor an analysis of the old code to the new. That check reads the
/// editor, so it holds for typed code only: a board arrives after every
/// stroke, and refusing a judgment for each would refuse nearly all of them.
pub fn apply_phase_judgment(state: &mut RuntimeState, text: &str) -> Vec<FrameworkEvidence> {
    // A call that failed is answered with nothing, where a judgment is always
    // JSON. Its window was marked read when it went out, so it is opened again
    // here: otherwise the candidate's last answer, read by a call that failed,
    // would wait for another turn that may never come.
    if text.trim().is_empty() {
        if state.judged_for != PhaseJudgeDue::EditorOnly {
            state.judged_transcript_lines = 0;
            state.judged_last_turn.clear();
        }
        state.judged_code.clear();
        return Vec::new();
    }
    let Ok(answer) = serde_json::from_str::<serde_json::Value>(text.trim()) else {
        return Vec::new();
    };

    // Read before the steps, since a refusal comes with none. Kept once seen: a
    // story told after declining is still recorded, and the probe the candidate
    // turned down stays unassessed rather than skipped.
    if state.behavioral_round_started
        && answer.get("declined").and_then(serde_json::Value::as_bool) == Some(true)
    {
        state.star_declined = true;
    }
    let Some(steps) = answer
        .get("steps")
        .and_then(serde_json::Value::as_array)
        .filter(|steps| !steps.is_empty())
    else {
        return Vec::new();
    };

    // Read once, as the call was asked: Coding recorded from this answer closes
    // the spoken steps to later calls, not to a restatement listed after it.
    // The transcript and the editor comparison are worked out only when a step
    // needs them, since most answers find nothing.
    let open = phase_judge_open(state);
    let mut turns = None;
    let mut code_current = None;
    let mut recorded = Vec::new();
    for step in steps.iter().take(JUDGEABLE.len()) {
        let (Some(id), Some(quote)) = (
            step.get("step").and_then(serde_json::Value::as_str),
            step.get("quote").and_then(serde_json::Value::as_str),
        ) else {
            continue;
        };
        let Some(phase) = JUDGEABLE.into_iter().find(|phase| phase_id(*phase) == id) else {
            continue;
        };
        if !open.contains(&id) || phases_evidenced(state, &[phase]) {
            continue;
        }
        if phase.needs_code()
            && !*code_current.get_or_insert_with(|| {
                let judged = TestedCode {
                    language: state.judged_language.clone(),
                    code: state.judged_code.clone(),
                };
                covers(&judged, &state.language, &state.code)
            })
        {
            continue;
        }
        let turns = turns.get_or_insert_with(|| speaker_turns(state));
        let Some(source) = quote_is_grounded(state, turns, phase, quote) else {
            continue;
        };
        let summary = step
            .get("summary")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|summary| !summary.is_empty());
        let quoted = format!(
            "\"{}\"",
            bounded_model_text(quote.trim(), MAX_PHASE_JUDGE_QUOTE_CHARS)
        );
        let summary = match summary {
            Some(summary) => format!(
                "{} Quote: {quoted}",
                bounded_model_text(summary, MAX_PHASE_JUDGE_SUMMARY_CHARS)
            ),
            None => format!("Quote: {quoted}"),
        };
        if let Ok((evidence, RecordOutcome::Added)) = record_evidence(
            state,
            &serde_json::json!({
                "phase": id,
                "source": evidence_source_id(source),
                "kind": "observed",
                "confidence": 70,
                "summary": summary,
            }),
            true,
        ) {
            recorded.push(evidence);
        }
    }
    recorded
}

/// One line of what a pause-time reviewer saw, bounded the way a summary is.
///
/// The text is model output and reaches the report prompt, so it is trimmed to
/// one line per observation and cut to a length: an idle-window call that
/// returns a paragraph, or a hundred of them, must not be able to crowd out the
/// transcript it sits beside.
pub fn record_interim_notes(state: &mut RuntimeState, text: &str) {
    let mut taken = 0usize;
    for line in text.lines() {
        // The prompt's own ceiling, enforced rather than trusted. Without it a
        // single degenerate response -- the answer is prose, so nothing but the
        // token cap bounds its line count -- walks the whole session's notes
        // out of the list one `remove(0)` at a time, and the report is then
        // written from one bad pause instead of the interview.
        if taken == MAX_INTERIM_LINES_PER_REVIEW {
            break;
        }

        // A bullet followed by a space, not every leading dash: the prompt asks
        // for "- ", and stripping the character on its own would edit "-1 is
        // the case they missed" down to a different claim.
        let line = line.trim();
        let line = line.strip_prefix("- ").unwrap_or(line).trim();
        if line.is_empty() {
            continue;
        }
        let line = bounded_model_text(line, MAX_INTERIM_LINE_CHARS);
        if state.interim_notes.iter().any(|held| held == &line) {
            continue;
        }
        taken += 1;
        if state.interim_notes.len() == MAX_INTERIM_NOTES {
            state.interim_notes.remove(INTERIM_OPENING_KEPT);
        }
        state.interim_notes.push(line);
    }
}
