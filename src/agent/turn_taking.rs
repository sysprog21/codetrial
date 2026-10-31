//! The candidate keeping the floor to think: what counts as asking for it, and
//! the one place a hold starts and ends.

use std::time::{Duration, Instant};

use super::{LifecycleTransition, RuntimeState};

/// How long a spoken request can stay provisional. Past the end of Gemini's
/// own turn, which followed the transcript by about four seconds in a measured
/// session and is what normally confirms it.
pub(crate) const THINKING_REQUEST_SETTLE: Duration = Duration::from_secs(6);

/// Whether the candidate holds the floor to think, and on what footing.
///
/// One value rather than a flag here and a "still provisional" flag in the
/// room loop: with two, every path that ended a hold had to remember both, the
/// ledger row, and what the page was told, and the paths that forgot were the
/// bugs. Every transition goes through the methods below.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ThinkingHold {
    #[default]
    Off,
    /// A spoken request still arriving. It suppresses replies like a hold,
    /// but later fragments of the same utterance can turn it back into
    /// reasoning, so nothing public has happened: no ledger row and no page
    /// state. `since` bounds how long it can stay undecided; see
    /// `settle_stale_request`.
    Requested { since: Instant },
    /// Declared, by the Thinking button or a spoken request whose utterance
    /// ended. `since` times the check-in.
    Held { since: Instant },
}

impl ThinkingHold {
    /// Replies and nudges are suppressed.
    pub fn is_active(self) -> bool {
        self != Self::Off
    }

    pub fn is_requested(self) -> bool {
        matches!(self, Self::Requested { .. })
    }

    /// The ledger and the page know about it.
    pub fn is_declared(self) -> bool {
        matches!(self, Self::Held { .. })
    }
}

impl RuntimeState {
    /// A spoken request has begun. Provisional; see
    /// [`ThinkingHold::Requested`].
    pub(crate) fn request_thinking(&mut self) {
        if self.thinking_hold == ThinkingHold::Off {
            self.thinking_hold = ThinkingHold::Requested {
                since: Instant::now(),
            };

            // The cooldown bounds replies a candidate can buy by toggling the
            // button. A hold they asked for aloud is not a toggle, and its
            // Continue is owed a reply however soon after the last one.
            self.thinking_released_at = None;
        }
        self.end_requested = false;
    }

    /// Declares a hold, from nothing or from a provisional request. Returns
    /// whether it is new, which is when the ledger records its start and the
    /// page is told.
    pub(crate) fn declare_thinking(&mut self, now: Instant, receipt: u64) -> bool {
        self.end_requested = false;
        if self.thinking_hold.is_declared() {
            return false;
        }
        self.thinking_hold = ThinkingHold::Held { since: now };
        self.evidence_ledger
            .record_lifecycle(receipt, LifecycleTransition::ThinkingStarted);
        self.thinking_notice = Some(true);
        true
    }

    /// The one way a hold ends. Only a declared hold records its end: a
    /// provisional one has no start in the ledger for an end to close, and the
    /// page never showed it. Returns whether it was declared, which is when
    /// the page is told.
    pub(crate) fn end_thinking(&mut self, receipt: u64) -> bool {
        let declared = self.thinking_hold.is_declared();
        self.thinking_hold = ThinkingHold::Off;
        if declared {
            self.evidence_ledger
                .record_lifecycle(receipt, LifecycleTransition::ThinkingEnded);
            self.thinking_notice = Some(false);
        }
        declared
    }

    /// Ends a declared hold that has run `THINKING_CHECK_IN_S` in silence, so
    /// the interviewer can check in. Timed from the hold's own start, so an
    /// earlier hold does not count toward it.
    pub(crate) fn claim_thinking_check_in(&mut self, now: Instant, receipt: u64) -> bool {
        let ThinkingHold::Held { since } = self.thinking_hold else {
            return false;
        };
        let limit = Duration::from_secs(super::THINKING_CHECK_IN_S);
        if self.ended || now.duration_since(since) < limit {
            return false;
        }
        self.end_thinking(receipt)
    }

    /// Declares a request nothing has confirmed or withdrawn in
    /// `THINKING_REQUEST_SETTLE`. Its confirmation is the end of Gemini's
    /// turn, and one that ended before the request's transcript arrived never
    /// comes; left undecided, the request kept the interviewer silent with no
    /// Continue on the page and no check-in. Returns whether it declared one.
    pub(crate) fn settle_stale_request(&mut self, now: Instant, receipt: u64) -> bool {
        let ThinkingHold::Requested { since } = self.thinking_hold else {
            return false;
        };
        if self.ended || now.duration_since(since) < THINKING_REQUEST_SETTLE {
            return false;
        }
        self.declare_thinking(now, receipt)
    }

    /// Whether the Thinking button ended a hold less than
    /// `THINKING_RELEASE_COOLDOWN` before `now`.
    pub(crate) fn released_recently(&self, now: Instant) -> bool {
        self.thinking_released_at
            .is_some_and(|at| now.duration_since(at) < super::THINKING_RELEASE_COOLDOWN)
    }

    /// A provisional request turned out to be reasoning, or its utterance can
    /// no longer end. Nothing public happened, so nothing is recorded. What the
    /// request dropped meanwhile stays owed: Gemini believes it delivered that
    /// output, and the reply asked for after it is told not to repeat what it
    /// already said, so without the note the dropped words are neither heard
    /// nor disowned. Returns whether there was one.
    pub(crate) fn withdraw_thinking_request(&mut self) -> bool {
        if !self.thinking_hold.is_requested() {
            return false;
        }
        self.thinking_hold = ThinkingHold::Off;
        true
    }

    /// Whether the interviewer must stay quiet for a thinking hold. The one
    /// question every reply, nudge and close asks. Not who has the floor,
    /// which is `RuntimeActivity::floor` and is often read right beside this.
    pub fn interviewer_silenced(&self) -> bool {
        self.thinking_hold.is_active()
    }

    /// Asks for the page to be told the declared state again, whether or not
    /// it changed: a page whose last acknowledgement was lost, or that has
    /// just rejoined, is corrected by it.
    pub(crate) fn announce_thinking(&mut self) {
        self.thinking_notice = Some(self.thinking_hold.is_declared());
    }

    /// What the page has not yet been told; the room loop publishes it once
    /// per event, so no path that changes the hold has to remember to.
    pub(crate) fn take_thinking_notice(&mut self) -> Option<bool> {
        self.thinking_notice.take()
    }

    /// The debt `thinking_debt` names has been delivered.
    pub(crate) fn clear_thinking_debt(&mut self) {
        let cold_brief = std::mem::take(&mut self.needs_cold_brief);
        if cold_brief {
            self.code_shown = self.code.clone();
        }

        // A held briefing, or a reply a replacement owed, is answered by the
        // next turn.
        self.recovery_reply_pending |= cold_brief || self.owed_reply_on_resume.is_some();
        self.owed_reply_on_resume = None;
        self.thinking_unheard_reply = false;
    }
}

/// A reply the hold dropped is still in the model's history. Without this it
/// can refer back to a hint the candidate never heard.
const THINKING_UNHEARD: &str = "[SYSTEM EVENT] Nothing you said while the candidate was thinking reached them. Do not refer to it or assume they heard any hint in it.";

/// What a hold leaves the interviewer owing, ahead of whatever ends it:
/// a cold replacement's briefing, the note that disowns what the hold
/// dropped, and a reply a replaced socket owed. `clear_thinking_debt` pays it.
fn thinking_debt(state: &RuntimeState) -> Vec<String> {
    let mut prompts = Vec::new();
    if state.needs_cold_brief {
        prompts.push(super::cold_restart(state));
    }
    prompts.extend(reply_debt(state));
    prompts
}

fn reply_debt(state: &RuntimeState) -> Vec<String> {
    let mut prompts = Vec::new();
    if state.thinking_unheard_reply {
        prompts.push(THINKING_UNHEARD.to_string());
    }

    // Held as the raw event and phrased only here, so a request that is itself
    // owed again is never wrapped inside another.
    if let Some(owed) = &state.owed_reply_on_resume {
        prompts.push(super::owed_reply(owed.as_deref()));
    }
    prompts
}

/// The reply obligations after a full cold briefing has already been added.
pub(crate) fn owed_context_after_briefing(state: &RuntimeState) -> String {
    reply_debt(state).join("\n")
}

/// Whether ending a hold owes the model anything beyond what ends it.
pub(crate) fn thinking_owes_context(state: &RuntimeState) -> bool {
    state.needs_cold_brief || state.thinking_unheard_reply || state.owed_reply_on_resume.is_some()
}

/// The debt alone, as one prompt.
pub(crate) fn owed_context(state: &RuntimeState) -> String {
    thinking_debt(state).join("\n")
}

/// `line`, behind whatever the hold left owed.
pub(crate) fn with_thinking_debt(state: &RuntimeState, line: &str) -> String {
    let mut prompts = thinking_debt(state);
    prompts.push(line.to_string());
    prompts.join("\n")
}

pub(crate) fn thinking_check_in(state: &RuntimeState) -> String {
    with_thinking_debt(
        state,
        &format!(
            "[SYSTEM EVENT] The candidate asked for thinking time and has been silent for {} minutes. Check in once, briefly and warmly: ask whether they want to talk through where they are or need more time. Do not give a hint.",
            super::THINKING_CHECK_IN_S / 60
        ),
    )
}

pub(crate) fn thinking_resume(state: &RuntimeState) -> String {
    with_thinking_debt(
        state,
        "[SYSTEM EVENT] The candidate is ready after thinking time. Respond briefly to their latest answer or invite them to continue, without repeating a question or giving an unsolicited hint.",
    )
}

const FILLERS: &[&str] = &[
    "hmm", "hm", "um", "uh", "erm", "ah", "okay", "ok", "well", "so", "right", "yeah",
];

/// The words of `text`, borrowed. Callers lowercase first, once.
fn words(text: &str) -> impl Iterator<Item = &str> {
    text.split(|ch: char| !ch.is_alphanumeric() && ch != '\'')
        .filter(|word| !word.is_empty())
}

/// Stops at the first word that is not a filler, so the whole turn so far is
/// neither copied nor, usually, read to the end.
fn resumes_after_thinking(text: &str) -> bool {
    words(text).any(|word| {
        !FILLERS
            .iter()
            .any(|filler| filler.eq_ignore_ascii_case(word))
    })
}

/// How far back from the end of a sentence a trailing request is looked for.
const TRAILING_WORDS: usize = 16;

/// Words that can open a clause without changing what it asks for.
const LEAD_INS: &[&str] = &[
    "please", "just", "and", "but", "now", "then", "actually", "sorry", "oh",
];

/// Question forms that ask for the same thing as the bare imperative.
const ASKS: &[&str] = &["can you", "could you", "would you", "will you"];

/// Requests complete on their own.
const PHRASES: &[&str] = &[
    "let me think",
    "let me see",
    "i need to think",
    "i need time to think",
];

/// Requests too short to tell from the subject matter unless they are the
/// whole sentence: "one second" asks for time, "the timeout is one second"
/// does not. A bare "wait" is one of these.
const SHORT_PHRASES: &[&str] = &[
    "hold on",
    "one moment",
    "one second",
    "one sec",
    "just a moment",
    "just a second",
    "wait",
];

/// Requests that name how long: each takes one of `DURATIONS`.
const STEMS: &[&str] = &[
    "give me",
    "i need",
    "let me take",
    "can i have",
    "can i take",
    "can i get",
    "could i have",
    "could i take",
    "may i have",
    "may i take",
];

const DURATIONS: &[&str] = &[
    "some time",
    "some more time",
    "more time",
    "a moment",
    "a minute",
    "a second",
    "a sec",
    "a bit",
    "a bit more time",
    "a little time",
    "a little more time",
    "a few minutes",
    "a few seconds",
    "a couple of minutes",
    "a couple minutes",
];

/// What may follow a request, or the start of it while fragments arrive.
const TAILS: &[&str] = &[
    "",
    "please",
    "a moment",
    "a minute",
    "a second",
    "a bit",
    "for a moment",
    "for a minute",
    "for a bit",
    "for a second",
    "about it",
    "about this",
    "about that",
    "it through",
    "this through",
    "to think",
    "to think about it",
    "to think this through",
    "to think for a moment",
    "to work this out",
];

fn is_lead_in(word: &str) -> bool {
    FILLERS.contains(&word) || LEAD_INS.contains(&word)
}

/// `words` with `phrase` taken off the front, if it starts with it.
fn strip_phrase<'a, 'b>(words: &'a [&'b str], phrase: &str) -> Option<&'a [&'b str]> {
    phrase.split(' ').try_fold(words, |rest, part| {
        let (first, tail) = rest.split_first()?;
        (*first == part).then_some(tail)
    })
}

fn skip_lead_ins<'a, 'b>(words: &'a [&'b str]) -> &'a [&'b str] {
    let start = words
        .iter()
        .position(|word| !is_lead_in(word))
        .unwrap_or(words.len());
    &words[start..]
}

/// What a request may say it is about before saying how long: "let me think
/// about it for a moment".
const OBJECTS: &[&str] = &["about it", "about this", "about that"];

/// Nothing but one of `TAILS`, or the first words of one, perhaps after one of
/// `OBJECTS`, and perhaps a closing "please".
fn is_tail(rest: &[&str]) -> bool {
    let rest = rest.strip_suffix(&["please"]).unwrap_or(rest);
    let after_object = OBJECTS
        .iter()
        .filter_map(|object| strip_phrase(rest, object));
    std::iter::once(rest).chain(after_object).any(|rest| {
        TAILS.iter().any(|tail| {
            let mut parts = words(tail);
            rest.iter().all(|word| parts.next() == Some(*word))
        })
    })
}

/// Whether `words` is a whole request: a known phrase, then nothing but one of
/// the endings a request takes. `short` admits `SHORT_PHRASES`, which only the
/// whole sentence may be; see there.
fn is_request(words: &[&str], short: bool) -> bool {
    let mut words = skip_lead_ins(words);
    if let Some(rest) = ASKS.iter().find_map(|ask| strip_phrase(words, ask)) {
        words = skip_lead_ins(rest);
    }

    // "Wait, let me think": a leading "wait" does not change what follows.
    let after_wait = strip_phrase(words, "wait");
    [Some(words), after_wait]
        .into_iter()
        .flatten()
        .any(|words| {
            let short = SHORT_PHRASES.iter().filter(|_| short);
            let bare = PHRASES
                .iter()
                .chain(short)
                .filter_map(|phrase| strip_phrase(words, phrase));
            let timed = STEMS
                .iter()
                .filter_map(|stem| strip_phrase(words, stem))
                .flat_map(|rest| {
                    DURATIONS
                        .iter()
                        .filter_map(move |duration| strip_phrase(rest, duration))
                });
            bare.chain(timed).any(is_tail)
        })
}

/// English only: a candidate speaking another language keeps the Thinking
/// button, but is not recognised asking aloud.
pub(crate) fn requests_thinking_time(text: &str) -> bool {
    // A request can follow an answer in the same utterance. Keep sentence
    // boundaries so a quoted phrase inside an answer does not become a request.
    //
    // A filler after the request, as its own sentence or its last word, does
    // not answer it: "Let me think. Hmm." and "let me think um" still ask. The
    // whole turn so far is read on every fragment, so without this the first
    // "hmm" before Gemini ends the turn withdrew the request. The same skip
    // passes over the empty piece a closing full stop leaves.
    let last_sentence = text
        .rsplit(['.', '?', '!'])
        .map(str::to_ascii_lowercase)
        .find(|sentence| !words(sentence).all(is_lead_in))
        .unwrap_or_default();
    let mut sentence = words(&last_sentence).collect::<Vec<_>>();
    while sentence.last().is_some_and(|word| is_lead_in(word)) {
        sentence.pop();
    }
    if is_request(&sentence, true) {
        return true;
    }

    // A clause of nothing but "please" or "okay" closes the request before it.
    let mut last_clause = last_sentence
        .rsplit([',', ';', ':'])
        .map(|clause| words(clause).collect::<Vec<_>>())
        .find(|clause| !clause.iter().all(|word| is_lead_in(word)))
        .unwrap_or_default();
    while last_clause.last().is_some_and(|word| is_lead_in(word)) {
        last_clause.pop();
    }
    if is_request(&last_clause, false) {
        return true;
    }

    // Transcripts often drop the comma before a trailing request. A lead-in
    // word stands in for it, so "a hash map please give me some time" asks
    // while "the user might say let me think" and "do not give me some time" do
    // not. Bounded, because the whole turn so far is rescanned on every
    // fragment: a request is short, and the lead-in before it is one word.
    // `is_request` skips the lead-in itself, so the candidates are the lead-ins
    // after the first word.
    let words = &sentence[sentence.len().saturating_sub(TRAILING_WORDS)..];
    (1..words.len())
        .filter(|&start| is_lead_in(words[start]))
        .any(|start| is_request(&words[start..], false))
}

/// What one more fragment of the candidate's utterance does to the hold:
/// `Some(true)` a request begins, `Some(false)` the hold or request ends. A
/// request is provisional while fragments arrive, so later reasoning withdraws
/// it; a declared hold ignores fillers but ends on real speech.
pub(crate) fn thinking_change(hold: ThinkingHold, text: &str) -> Option<bool> {
    match hold {
        ThinkingHold::Off => requests_thinking_time(text).then_some(true),
        ThinkingHold::Requested { .. } => (!requests_thinking_time(text)).then_some(false),

        // Fillers first: they are most of what a candidate thinking aloud says,
        // and they settle it without the phrase matcher.
        ThinkingHold::Held { .. } => {
            (resumes_after_thinking(text) && !requests_thinking_time(text)).then_some(false)
        }
    }
}

#[cfg(test)]
#[path = "../../tests/unit/agent/turn_taking.rs"]
mod tests;
