//! A minimum amount of retained speech before a report may assess
//! communication.

use super::CANCELLED_INTERVIEWER_OUTPUT;
use super::transcript::{mark_unrecognized_turns, speaker_speech, tail_start};
use super::{CANDIDATE_SPEAKER, INTERVIEWER_SPEAKER, MAX_TRANSCRIPT_BYTES, UNRECOGNIZED_TURN};

// Uncalibrated starting values, not a measure of correctness or speaking
// quality.
const MIN_TOTAL_WORDS: usize = 20;
const MIN_TURN_WORDS: usize = 8;
const MIN_ECHO_WORDS: usize = 5;
const ECHO_LOOKBACK: usize = 2;

const FILLERS: &[&str] = &[
    "uh", "uhm", "um", "hm", "hmm", "mhm", "mm", "huh", "er", "erm", "ah",
];
const ACKNOWLEDGEMENTS: &[&[&str]] = &[
    &["yes"],
    &["yeah"],
    &["yep"],
    &["no"],
    &["ok"],
    &["okay"],
    &["sure"],
    &["right"],
    &["got", "it"],
    &["i", "see"],
    &["thank", "you"],
    &["makes", "sense"],
    &["sounds", "good"],
    &["all", "right"],
    &["alright"],
    &["thanks"],
    &["yup"],
    &["nope"],
];
const NON_SPEECH: &[&[&str]] = &[
    &["cough"],
    &["coughing"],
    &["noise"],
    &["background", "noise"],
    &["silence"],
    &["inaudible"],
    &["unintelligible"],
    &["laughter"],
];
const NON_SPEECH_ANNOTATIONS: &[&[&str]] = &[
    &["coughs"],
    &["laughs"],
    &["laughing"],
    &["clears", "throat"],
    &["music"],
    &["breathing"],
];

/// Report-only eligibility, never serialized or used to control a live turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommunicationEvidence {
    NoSpeech,
    Insufficient,
    Sufficient,
}

/// Classifies the same masked, byte-bounded lines that the report can read.
/// The original lines only distinguish absence from evidence lost to masking
/// or truncation; they cannot contribute words after being dropped.
pub fn communication_evidence(lines: &[String]) -> CommunicationEvidence {
    classify(lines, None, None)
}

pub(crate) fn report_communication_evidence(
    state: &super::RuntimeState,
    lines: &[String],
) -> CommunicationEvidence {
    let round_mark = state
        .behavioral_round_started
        .then(|| super::behavioral_round_start(state));
    classify(lines, Some(&state.transcript), round_mark)
}

fn classify(
    lines: &[String],
    raw: Option<&[String]>,
    round_mark: Option<usize>,
) -> CommunicationEvidence {
    let masked = mark_unrecognized_turns(lines);

    // An oversized final line is shown as an unattributed suffix by
    // transcript_tail. It cannot safely establish whose words those are.
    let start = tail_start(&masked, MAX_TRANSCRIPT_BYTES);
    let mut interviewers = std::collections::VecDeque::new();
    let mut total = 0;
    let mut longest = 0;
    // The inserted round marker has no corresponding raw transcript line.
    let mut raw_lines = raw.into_iter().flatten();
    let aligned = masked.iter().enumerate().map(|(index, line)| {
        let original = (Some(index) != round_mark)
            .then(|| raw_lines.next())
            .flatten();
        (line, original)
    });
    for (line, original) in aligned.skip(start) {
        if let Some(text) = speaker_speech(line, INTERVIEWER_SPEAKER) {
            let uncertain = text == CANCELLED_INTERVIEWER_OUTPUT;
            let text = if uncertain {
                let Some(text) =
                    original.and_then(|line| speaker_speech(line, INTERVIEWER_SPEAKER))
                else {
                    continue;
                };
                text
            } else {
                text
            };
            let normalized = words(text);
            if normalized.is_empty() {
                continue;
            }
            if interviewers.len() == ECHO_LOOKBACK {
                interviewers.pop_front();
            }
            interviewers.push_back(normalized);
            continue;
        }
        let Some(text) = speaker_speech(line, CANDIDATE_SPEAKER) else {
            continue;
        };
        if text.trim() == UNRECOGNIZED_TURN {
            continue;
        }
        let words = words(text);

        // This is uncertain attribution, not acoustic echo detection. Genuine
        // repetition also matches. Partial, paraphrased or older echo can pass;
        // exclusion changes eligibility only, never the transcript or playback.
        if words.len() >= MIN_ECHO_WORDS
            && interviewers.iter().any(|previous: &Vec<String>| {
                previous.windows(words.len()).any(|span| span == words)
            })
        {
            continue;
        }
        if only_markers(&words, &[NON_SPEECH, ACKNOWLEDGEMENTS]) {
            continue;
        }
        total += words.len();
        longest = longest.max(words.len());
        if total >= MIN_TOTAL_WORDS && longest >= MIN_TURN_WORDS {
            return CommunicationEvidence::Sufficient;
        }
    }
    if lines.iter().any(|line| {
        speaker_speech(line, CANDIDATE_SPEAKER).is_some_and(|text| !text.trim().is_empty())
    }) {
        CommunicationEvidence::Insufficient
    } else {
        CommunicationEvidence::NoSpeech
    }
}

// A marker inside a substantive sentence is still part of that sentence.
fn only_markers(mut words: &[impl AsRef<str>], marker_lists: &[&[&[&str]]]) -> bool {
    while !words.is_empty() {
        let Some(marker) = marker_lists
            .iter()
            .flat_map(|list| list.iter())
            .find(|marker| {
                words.get(..marker.len()).is_some_and(|prefix| {
                    prefix
                        .iter()
                        .zip(marker.iter())
                        .all(|(word, expected)| word.as_ref() == *expected)
                })
            })
        else {
            return false;
        };
        words = &words[marker.len()..];
    }
    true
}

fn is_non_speech_annotation(label: &str) -> bool {
    let label = label.to_ascii_lowercase();
    let parts: Vec<_> = word_parts(&label).collect();
    !parts.is_empty() && only_markers(&parts, &[NON_SPEECH, NON_SPEECH_ANNOTATIONS])
}

// A word is a maximal Unicode alphanumeric run. Punctuation, including an
// apostrophe or underscore, separates words.
fn word_parts(text: &str) -> impl Iterator<Item = &str> {
    text.split(|character: char| !character.is_alphanumeric())
        .filter(|word| !word.is_empty())
}

fn words(text: &str) -> Vec<String> {
    let mut speech = String::with_capacity(text.len());
    let mut open = Vec::new();
    for (index, character) in text.char_indices() {
        match character {
            '[' | '(' => open.push((index, speech.len(), character)),
            ']' | ')' => {
                if let Some(&(start, output, opener)) = open.last()
                    && matches!((opener, character), ('[', ']') | ('(', ')'))
                {
                    open.pop();
                    if is_non_speech_annotation(&text[start..index]) {
                        speech.truncate(output);
                        speech.push(' ');
                        continue;
                    }
                }
            }
            _ => {}
        }
        speech.push(character);
    }
    word_parts(&speech)
        // Unicode lowercase can introduce a combining mark, so normalize each
        // word only after finding its original alphanumeric boundaries.
        .map(str::to_lowercase)
        .filter(|word| !is_filler(word))
        .collect()
}

fn is_filler(word: &str) -> bool {
    // Accept extra repetitions of a filler's final letter without changing the
    // tokens used for counting or matching substantive speech.
    FILLERS.iter().any(|filler| {
        let last = filler.as_bytes()[filler.len() - 1];
        word.strip_prefix(filler)
            .is_some_and(|rest| rest.bytes().all(|byte| byte == last))
    })
}
