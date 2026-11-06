//! The transcript as the report and the prompts read it: the bounded tail, the
//! turns the speech recognizer could not make out, and the speaker prefixes.

// This module reads the hub's imports and types through `use super::*`.
#[cfg_attr(
    not(test),
    expect(
        clippy::wildcard_imports,
        reason = "the module reads its hub through super::*"
    )
)]
use super::*;

/// How much conversation the report prompt may carry. A 90-minute interview
/// runs to roughly 40k characters of speech, so this never fires for a real
/// session; it exists so a pathological one cannot grow the prompt without a
/// ceiling. The tail is what matters to a grader: the closing reasoning, not
/// the opening pleasantries.
///
/// Bytes, because that is what bounds the request. A transcript in a language
/// that spends three bytes a character therefore carries fewer characters, not
/// more bytes than it can afford.
pub const MAX_TRANSCRIPT_BYTES: usize = 60_000;

/// Joins the session transcript for the report prompt, keeping the most recent
/// entries when the whole thing would not fit.
pub fn transcript_for_report(lines: &[String]) -> String {
    transcript_tail(&mark_unrecognized_turns(lines), MAX_TRANSCRIPT_BYTES)
}

/// Whether `transcript_for_report` leaves out the opening of these lines.
/// Asked of the lines rather than of the text it returns, which carries the
/// candidate's own words and so can spell the omission notice itself.
pub(crate) fn report_transcript_cut(lines: &[String]) -> bool {
    tail_start(&mark_unrecognized_turns(lines), MAX_TRANSCRIPT_BYTES) > 0
}

/// What an assessment reads in place of a candidate turn the recognizer did
/// not return as English. The report prompt names it, so a change here that
/// left the prompt describing the old marker fails
/// `prompts_match_frozen_fixture`.
pub const UNRECOGNIZED_TURN: &str = "(this turn was not recognized as English and is left out)";

/// `lines` with every candidate turn written mostly in a non-Latin script
/// replaced by `UNRECOGNIZED_TURN`, for the passes that assess the candidate.
///
/// The interview is in English, so such a turn is the recognizer's output
/// rather than what the candidate said: fluent Mandarin about clothing sizes
/// came back for an English answer about delimiters. Shown the text, the
/// report model cited it, and once told not to, still rewrote it into an
/// improvement about staying on topic, however each wording was refused. A
/// turn it never reads cannot be coached. The live interviewer, the replay and
/// the stored transcript keep what the recognizer produced; only assessment is
/// spared it.
///
/// Most of the letters, not any: a sentence that quotes a string in another
/// script is still English, and accented Latin is still Latin. English that
/// the recognizer turned into Spanish or into unrelated English stays, and the
/// prompts' uncertainty rules are what cover it.
pub fn mark_unrecognized_turns(lines: &[String]) -> Vec<String> {
    lines
        .iter()
        .map(|line| {
            if is_unrecognized_turn(line) {
                format!("{CANDIDATE_SPEAKER}: {UNRECOGNIZED_TURN}")
            } else {
                line.clone()
            }
        })
        .collect()
}

/// Whether the candidate has said anything the recognizer did not return
/// mostly in a non-Latin script. A turn `mark_unrecognized_turns` hides is not
/// evidence that the discussion of the exercise has begun.
pub(crate) fn has_recognized_candidate_turn(lines: &[String]) -> bool {
    lines
        .iter()
        .any(|line| candidate_speech(line).is_some_and(|speech| !mostly_non_latin(speech)))
}

/// Whether a learner's speech, without a speaker prefix, is one
/// `mark_unrecognized_turns` would hide. Task mode keeps its turns by id rather
/// than as transcript lines, so it asks of the text alone.
pub(crate) fn is_unrecognized_speech(speech: &str) -> bool {
    mostly_non_latin(speech)
}

/// What an assessment reads for a learner's speech without a speaker prefix:
/// the speech itself, or `UNRECOGNIZED_TURN` for one `mark_unrecognized_turns`
/// would hide.
pub(crate) fn assessed_speech(speech: &str) -> &str {
    if mostly_non_latin(speech) {
        UNRECOGNIZED_TURN
    } else {
        speech
    }
}

/// Whether `line` is a candidate turn `mark_unrecognized_turns` hides.
pub(super) fn is_unrecognized_turn(line: &str) -> bool {
    candidate_speech(line).is_some_and(mostly_non_latin)
}

/// What the candidate said in `line`, if it is their turn.
fn candidate_speech(line: &str) -> Option<&str> {
    speaker_speech(line, CANDIDATE_SPEAKER)
}

/// What `speaker` said in `line`, if it is their turn.
pub(crate) fn speaker_speech<'a>(line: &'a str, speaker: &str) -> Option<&'a str> {
    line.strip_prefix(speaker)
        .and_then(|rest| rest.strip_prefix(": "))
}

/// More than two non-Latin letters as well as a majority, so a turn that is
/// only a symbol the question is about, "theta" come back as a Greek letter,
/// survives: the prompts call that Unicode, not a recognition error, and a
/// hidden turn gives them nothing to apply it to. The sentence-length turns the
/// recognizer invents carry many more.
fn mostly_non_latin(speech: &str) -> bool {
    let (latin, other) = speech
        .chars()
        .filter(|character| character.is_alphabetic())
        .fold((0usize, 0usize), |(latin, other), character| {
            // ASCII, Latin-1 and Latin Extended-A/B, and Latin Extended
            // Additional, where Vietnamese lives.
            if character.is_ascii_alphabetic()
                || ('\u{00C0}'..='\u{024F}').contains(&character)
                || ('\u{1E00}'..='\u{1EFF}').contains(&character)
            {
                (latin + 1, other)
            } else {
                (latin, other + 1)
            }
        });
    other > latin && other > 2
}

/// The newest entries that fit in `budget` bytes, joined.
///
/// `budget` bounds the entries, and `EARLIER_OMITTED` is added above them when
/// any were dropped, so a trimmed tail is that much longer than the budget.
/// The same trade as `code_head`: the notice is what stops the reader taking
/// the opening as missing, and paying for it in dropped conversation would be
/// the wrong economy.
///
/// Two callers want the same tail against different ceilings: the report prompt
/// above, and the briefing a cold-restarted interviewer is rebuilt from. The
/// beginning is the least useful part to lose in both, and the notice below is
/// what stops either reader from taking the opening as missing rather than
/// dropped. Empty in, empty out: naming that case is the caller's, because a
/// report says nothing about it and a restart has to.
pub fn transcript_tail(lines: &[String], budget: usize) -> String {
    let start = tail_start(lines, budget);
    let mut kept = lines[start..]
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    if start > 0 {
        // One line can outrun the whole budget on its own. Dropping it would
        // answer a cold restart with nothing but the notice, so keep the end of
        // it: that is the part the conversation stopped in the middle of.
        if kept.is_empty() {
            kept.push(tail_within(&lines[start - 1], budget));
        }
        kept.insert(0, EARLIER_OMITTED);
    }
    kept.join("\n")
}

/// What stands in for the lines a bounded tail dropped, so a reader takes the
/// opening as cut rather than missing.
pub(crate) const EARLIER_OMITTED: &str = "(earlier conversation omitted)";

/// The first of the whole lines `transcript_tail` keeps, each paid for with
/// its separator. A cold restart asks this rather than counting for itself,
/// so what it believes the replacement can see is what the replacement is
/// shown.
pub(super) fn tail_start(lines: &[String], mut budget: usize) -> usize {
    for (index, line) in lines.iter().enumerate().rev() {
        match budget.checked_sub(line.len() + 1) {
            Some(remaining) => budget = remaining,
            None => return index + 1,
        }
    }
    0
}

/// The longest suffix of `line` that starts on a character boundary and fits in
/// `budget` bytes.
///
/// Written as that sentence rather than as a walk from `len - budget`, which is
/// the same answer arrived at by arithmetic that has two ways to be wrong: a
/// step in the other direction also lands on a boundary and quietly returns
/// more than `budget`, and a step that fails to advance never terminates. The
/// suffix a candidate speaking a three-byte-per-character language gets is up
/// to two bytes shorter than one speaking ASCII, which is the cost of the slice
/// being valid.
pub(super) fn tail_within(line: &str, budget: usize) -> &str {
    line.char_indices()
        .map(|(start, _)| &line[start..])
        .find(|suffix| suffix.len() <= budget)
        .unwrap_or("")
}
