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
/// were verified rather than only the judge's account of them. A comment's
/// words reach the report only through an `editor_snapshot` Repeat row's
/// quote, so one longer than the row keeps is refused for Repeat rather than
/// cut; the judge's prompt names the bound.
const MAX_PHASE_JUDGE_SUMMARY_CHARS: usize = 120;
pub(crate) const MAX_PHASE_JUDGE_QUOTE_CHARS: usize = 110;
/// The most repeat entries one answer is read for: the judge quotes a comment
/// longer than a row keeps in parts, and the code it is shown holds at most
/// this many parts of that length.
const MAX_REPEAT_PARTS: usize = PHASE_JUDGE_CODE_BYTES.div_ceil(MAX_PHASE_JUDGE_QUOTE_CHARS);

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
        .filter(|phase| {
            !phases_evidenced(state, &[*phase])
                || (*phase == FrameworkPhase::Repeat && restatement_open(state))
        })
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

/// Whether a comment typed now was typed before the candidate moved on: at an
/// editor, while no step but Repeat is recorded. The report credits a rule
/// written as a comment only when a `repeat` row from `editor_snapshot` quotes
/// it, so a comment is recorded for Repeat, as the first restatement or added
/// to a recorded one, only while this holds; a skipped row is no step taken.
/// A late spoken restatement is still recorded, from `candidate_speech`, so its
/// words can match a comment typed after this closed.
fn restatement_open(state: &RuntimeState) -> bool {
    !state.interview_mode.is_whiteboard()
        && !state
            .framework_evidence
            .iter()
            .any(|item| item.phase != FrameworkPhase::Repeat && item.kind != EvidenceKind::Skipped)
}

/// What the judge is told about an open Repeat, so it is not invited to quote
/// a comment `quote_is_grounded` would refuse: once Repeat is recorded only a
/// comment adds to it, and once another step is recorded first only speech
/// records it. A whiteboard has no comment to speak of.
fn repeat_cue(state: &RuntimeState, open: &[&str]) -> RepeatCue {
    if !open.contains(&"repeat") {
        RepeatCue::None
    } else if phases_evidenced(state, &[FrameworkPhase::Repeat]) {
        RepeatCue::Reopened
    } else if !state.interview_mode.is_whiteboard() && !restatement_open(state) {
        RepeatCue::SpeechOnly
    } else {
        RepeatCue::None
    }
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
        repeat: repeat_cue(state, &open),
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

/// The fewest of a starter comment line's words a candidate's comment line
/// must keep, in order, to be read as that line edited rather than as their
/// own sentence: three, and four in five of the starter line's words, and
/// more than half of the words from the first kept to the last. A shorter
/// starter line, such as "@return {number}", is a phrase a restatement of the
/// output says in passing, and a sentence that keeps less of a longer one,
/// such as "I return an array of host groups, each of size k" beside "Return
/// an array of arrays of size *returnSize", or spreads what it keeps across
/// itself, such as "For a binary tree, return the label of each node seen
/// from the left" beside "Definition for a binary tree node.", says it in the
/// candidate's own words.
const MIN_STARTER_WORDS_KEPT: usize = 3;

/// A comment line's words, `quote_words` spelling, with each word it keeps from
/// a starter comment line it edits replaced by a `|`, which no quote's words
/// contain, so no quote can match across or into the starter's words: a line
/// left as the starter wrote it, however short, holds nothing of the
/// candidate's, and one they added a word or two to, or took words out of,
/// holds only the words they added. The longer starter lines are matched
/// first, so a shorter one that begins a longer one does not take its words
/// and leave the rest too few to match. A starter line typed twice is cut
/// twice.
fn without_starter(words: &str, given: &[String]) -> String {
    let unchanged = given.iter().any(|line| line == words);
    let words = words.split(' ').collect::<Vec<_>>();
    let mut starter = vec![unchanged; words.len()];
    let mut given = given
        .iter()
        .map(|line| line.split(' ').collect::<Vec<_>>())
        .collect::<Vec<_>>();
    given.sort_by_key(|line| std::cmp::Reverse(line.len()));
    for line in given {
        let least = MIN_STARTER_WORDS_KEPT.max((line.len() * 4).div_ceil(5));
        loop {
            let free = (0..words.len())
                .filter(|at| !starter[*at])
                .collect::<Vec<_>>();
            let text = free.iter().map(|at| words[*at]).collect::<Vec<_>>();
            let Some(kept) = edited_starter_line(&line, &text, least) else {
                break;
            };

            // Each find is at least `least` free words, so every pass marks
            // some and the loop ends; this stops it anyway should a pass mark
            // none, rather than finding the same words for good.
            let mut marked = false;
            for at in kept {
                marked |= !std::mem::replace(&mut starter[free[at]], true);
            }
            if !marked {
                break;
            }
        }
    }
    let marked = words.iter().zip(&starter);
    let marked = marked.map(|(word, given)| if *given { "|" } else { word });
    marked.collect::<Vec<_>>().join(" ")
}

/// Where in `text` `line` falls edited: at least `least` of its words, kept in
/// their order, that are more than half of the words from the first of them to
/// the last, so a word or two added inside the line is still the line while a
/// sentence that spreads its words across itself is not. Of the stretches of
/// `text` that hold such a run, the one keeping the most words wins, the
/// earliest and then the shortest of those, so an edited line is cut whole
/// rather than a part of it that already passes.
fn edited_starter_line(line: &[&str], text: &[&str], least: usize) -> Option<Vec<usize>> {
    // A stretch longer than this has its first and last kept word too far apart
    // for even the whole line to be most of it.
    let widest = (2 * line.len()).saturating_sub(1);
    let mut best: Option<(usize, usize, usize)> = None;
    for start in 0..text.len() {
        let stretch = &text[start..text.len().min(start + widest)];
        for (width, kept) in kept_by_width(line, stretch).into_iter().enumerate() {
            if kept >= least && kept * 2 > width && best.is_none_or(|(most, _, _)| kept > most) {
                best = Some((kept, start, width));
            }
        }
    }
    let (_, start, width) = best?;
    let kept = kept_in_order(line, &text[start..start + width]);
    Some(kept.into_iter().map(|at| start + at).collect())
}

/// How many of `line`'s words, kept in their order, the first `width` words of
/// `text` hold, for each `width` from none to all of them.
fn kept_by_width(line: &[&str], text: &[&str]) -> Vec<usize> {
    let mut row = vec![0usize; text.len() + 1];
    for word in line {
        let mut next = vec![0usize; text.len() + 1];
        for (j, other) in text.iter().enumerate() {
            next[j + 1] = if word == other {
                row[j] + 1
            } else {
                row[j + 1].max(next[j])
            };
        }
        row = next;
    }
    row
}

/// Where in `text` a longest run of `line`'s words, kept in their order, falls:
/// the word-level longest common subsequence, as a diff reads an edited line.
fn kept_in_order(line: &[&str], text: &[&str]) -> Vec<usize> {
    let mut longest = vec![vec![0usize; text.len() + 1]; line.len() + 1];
    for (i, word) in line.iter().enumerate().rev() {
        for (j, other) in text.iter().enumerate().rev() {
            longest[i][j] = if word == other {
                longest[i + 1][j + 1] + 1
            } else {
                longest[i + 1][j].max(longest[i][j + 1])
            };
        }
    }

    // Walked one text word at a time, skipping the line's words a longest run
    // does not need there, so it ends after the text however the steps are
    // counted: a counter that failed to advance left an open loop spinning.
    let (mut i, mut kept) = (0, Vec::new());
    for (j, other) in text.iter().enumerate() {
        let needed =
            (i..line.len()).find(|&at| line[at] == *other || longest[at + 1][j] != longest[at][j]);
        let Some(at) = needed else {
            break;
        };
        i = at;
        if line[at] == *other {
            kept.push(j);
            i = at + 1;
        }
    }
    kept
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
    /// The transcript line it opens on.
    pub(super) start: usize,
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
    let from = judged_from(state);
    for (index, line) in state.transcript[from..].iter().enumerate() {
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
                start: from + index,
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
/// Speech also gives the line the first candidate turn that said it opens on.
/// `own` holds the candidate's comment words once a step has needed them, so
/// an answer cuts the starter out of the code at most once.
fn quote_is_grounded(
    state: &RuntimeState,
    turns: &[Turn],
    own: &std::cell::OnceCell<String>,
    phase: FrameworkPhase,
    quote: &str,
) -> Option<(EvidenceSource, Option<usize>)> {
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
    let editor = Some((EvidenceSource::EditorSnapshot, None));
    if phase == FrameworkPhase::Coding {
        return editor.filter(|_| typed(&state.code) && !typed(starter(state, &state.language)));
    }

    // A candidate whose speech was not recognized is asked to type their
    // explanation as a comment, and at an editor without the evidence tool
    // nobody else can record it from there. A hand trace stays spoken: Test
    // without a run is recorded as the candidate's speech, which is how the
    // report tells tracing from testing. The starter's comment lines are cut
    // out of the candidate's before the lines are joined, an edited one
    // included, so its words never ground a quote or help one reach the floor.
    // A comment counts for Repeat only while no other step is recorded, so
    // after that a restatement both said and typed is speech, and the report
    // reads its `candidate_speech` row as speech, not as dating the comment.
    // The report reads a comment only through an `editor_snapshot` Repeat row's
    // quote, so one the row would cut is not a Repeat comment: a rule past the
    // cut would be recorded as left out. Speech reaches the report whole in the
    // transcript while the report's tail of it still holds the turn; a long
    // interview can drop the opening restatement from it, a known limit.
    let whole = bounded_model_text(quote.trim(), MAX_PHASE_JUDGE_QUOTE_CHARS + 1)
        .chars()
        .count()
        <= MAX_PHASE_JUDGE_QUOTE_CHARS;
    if !phase.is_star()
        && phase != FrameworkPhase::Test
        && !state.interview_mode.is_whiteboard()
        && (phase != FrameworkPhase::Repeat || (whole && restatement_open(state)))
        && format!(" {} ", own.get_or_init(|| own_comment_words(state))).contains(&needle)
    {
        return editor;
    }
    let mut said = turns.iter().filter(|turn| turn.words.contains(&needle));
    let first = said.clone().next()?;
    let spoken = said.find(|turn| turn.speaker == CANDIDATE_SPEAKER)?;

    // A trace the interviewer walked first, or a story they told back before
    // the candidate did, is the candidate agreeing too.
    let echo = (phase.is_star()
        || matches!(
            phase,
            FrameworkPhase::Algorithm | FrameworkPhase::Optimizations | FrameworkPhase::Test
        ))
        && first.speaker == INTERVIEWER_SPEAKER;
    (!echo).then_some((EvidenceSource::CandidateSpeech, Some(spoken.start)))
}

/// The candidate's comment lines, `quote_words` spelling, joined with the
/// starter's words cut out of them. The cut reads every comment line against
/// every starter comment line, so it is made once for a judge answer rather
/// than once for each of its entries. Only the head of the editor the judge
/// is shown is read: a comment past it is one the judge never saw, and the
/// cut grows with the square of a line's words, so a long pasted line would
/// otherwise hold the session for a second or more.
fn own_comment_words(state: &RuntimeState) -> String {
    let given = typed_comments(&state.language, starter(state, &state.language));
    let given = given
        .lines()
        .map(quote_words)
        .filter(|words| !words.is_empty())
        .collect::<Vec<_>>();
    let shown = &state.code[..state.code.floor_char_boundary(PHASE_JUDGE_CODE_BYTES)];
    typed_comments(&state.language, shown)
        .lines()
        .map(|line| without_starter(&quote_words(line), &given))
        .filter(|words| !words.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
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
    // needs them, since most answers find nothing. The code does not change
    // while the answer is applied, so the starter is cut out of its comments
    // once, by the first step that reads them.
    let open = phase_judge_open(state);
    let mut turns = None;
    let own = std::cell::OnceCell::new();
    let mut code_current = None;
    let mut recorded = Vec::new();

    // A first restatement is the first Repeat entry actually recorded, so one
    // the server refuses does not take its place: while Repeat is unrecorded
    // the answer's Repeat entries are tried first, in order, and once anything
    // but a Repeat comment is recorded the Repeat entries still waiting go
    // behind the other steps, where they start when Repeat was recorded before.
    // Another step recorded from the same answer then moves the candidate on
    // whatever order the judge listed the steps in, after the answer's comment
    // entries when its restatement is a comment.
    let repeat = |step: &&serde_json::Value| {
        step.get("step").and_then(serde_json::Value::as_str) == Some("repeat")
    };

    // Read up to one entry for each other step, and for Repeat every part of
    // the longest comment the judge is shown, quoted in parts.
    let (mut repeats, mut others): (Vec<_>, Vec<_>) = steps.iter().partition(repeat);
    repeats.truncate(MAX_REPEAT_PARTS);
    others.truncate(JUDGEABLE.len());
    let mut queue =
        std::collections::VecDeque::from(if phases_evidenced(state, &[FrameworkPhase::Repeat]) {
            [others, repeats].concat()
        } else {
            [repeats, others].concat()
        });
    while let Some(step) = queue.pop_front() {
        let (Some(id), Some(quote)) = (
            step.get("step").and_then(serde_json::Value::as_str),
            step.get("quote").and_then(serde_json::Value::as_str),
        ) else {
            continue;
        };
        let Some(phase) = JUDGEABLE.into_iter().find(|phase| phase_id(*phase) == id) else {
            continue;
        };

        // Only Repeat is judged again once recorded, and then only from a
        // comment, below.
        let again = phases_evidenced(state, &[phase]);
        if !open.contains(&id) || (again && phase != FrameworkPhase::Repeat) {
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
        let Some((source, spoken_at)) = quote_is_grounded(state, turns, &own, phase, quote) else {
            continue;
        };

        // A comment counts for Repeat only while no other step is recorded, one
        // recorded earlier in this answer included, and `quote_is_grounded`
        // reads it as one only then. The queue settles which comes first: while
        // Repeat is unrecorded its entries go ahead, so a first restatement
        // typed as a comment counts beside the answer's other steps; when it
        // was recorded before this answer they go behind, so those steps close
        // it. Once Repeat is recorded only a comment adds to it: speech is the
        // restatement already recorded, and a quote a Repeat row already holds
        // adds nothing. A late spoken restatement is still recorded.
        let comment = source == EvidenceSource::EditorSnapshot;
        if again
            && (!comment
                || restatement_quoted(state, own.get_or_init(|| own_comment_words(state)), quote))
        {
            continue;
        }
        let summary = step
            .get("summary")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|summary| !summary.is_empty());

        // A double quote inside becomes a single one, so the quote is always
        // what the row's last two double quotes hold, whatever it says.
        let quoted = format!(
            "\"{}\"",
            bounded_model_text(quote.trim(), MAX_PHASE_JUDGE_QUOTE_CHARS).replace('"', "'")
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
            let latest = &mut state.hint_attempt.quoted_turn_start;
            *latest = (*latest).max(spoken_at);
            recorded.push(evidence);

            // A comment recorded for Repeat keeps the answer's other Repeat
            // entries ahead of its other steps: they may be the rest of a
            // comment the judge quoted in parts, typed before the candidate
            // moved on as surely as this part was, and only a comment can add
            // to it now. A spoken one sends them behind. Another step never
            // finds one still ahead of it, since they are tried first while
            // Repeat is unrecorded and last once it is.
            if !comment {
                let (left, rest): (Vec<_>, Vec<_>) = queue.drain(..).partition(repeat);
                queue.extend(rest.into_iter().chain(left));
            }
        }
    }
    recorded
}

/// Whether the Repeat rows already quote every word of the comment `quote`
/// holds: each row's quote is placed wherever it falls in `comment`, the
/// candidate's comment words as `quote_is_grounded` read them, and the quote is
/// refused when one place it falls is covered throughout. Placed rather than
/// joined, since the judge may quote a comment in parts that overlap, and
/// recorded in any order, by one answer or several, with a spoken row between
/// them, so rows read end to end repeat the overlap at a seam or put the parts
/// out of order, and a quote across them would be recorded again. A spoken row
/// covers the comment words it says too. A comment is recorded for Repeat only
/// when the row keeps it whole, so it needs no cut to be found. Only the row's
/// quote is read: the judge's summary beside it is not the candidate's words,
/// and either may contain "Quote: ", so the quote is found by the double quotes
/// around it, which it never contains.
fn restatement_quoted(state: &RuntimeState, comment: &str, quote: &str) -> bool {
    let comment = comment.split(' ').collect::<Vec<_>>();
    let places = |words: &str| places_in(&comment, &words.split(' ').collect::<Vec<_>>());
    let mut covered = vec![false; comment.len()];
    let rows = state
        .framework_evidence
        .iter()
        .filter(|item| item.phase == FrameworkPhase::Repeat)
        .filter_map(|item| {
            item.summary
                .strip_suffix('"')
                .and_then(|summary| summary.rsplit_once('"'))
                .map(|(_, quoted)| quote_words(quoted))
        })
        .filter(|words| !words.is_empty());
    for row in rows {
        for place in places(&row) {
            covered[place].fill(true);
        }
    }
    places(&quote_words(quote))
        .into_iter()
        .any(|place| covered[place].iter().all(|word| *word))
}

/// Every stretch of `text` that reads `words`, word for word.
fn places_in(text: &[&str], words: &[&str]) -> Vec<std::ops::Range<usize>> {
    text.windows(words.len())
        .enumerate()
        .filter(|(_, window)| *window == words)
        .map(|(start, _)| start..start + words.len())
        .collect()
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

#[cfg(test)]
#[path = "../../tests/unit/agent/phase_judge.rs"]
mod tests;
