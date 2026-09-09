//! Whose turn it is: the state an interview is a sequence of.
//!
//! Data and the rules over it, nothing that drives the room. Who holds the
//! floor, when the candidate last typed or spoke, which prompt is due, and
//! whether a wrap-up is owed at all.
//!
//! The procedures that end a turn stayed in the room loop, because every one of
//! them borrows its `GeminiEventContext`. They read as turn logic, but a module
//! that has to reach back into its parent for the type it operates on is not a
//! seam; it is the same code in two files. This depends on `crate::agent` and
//! on nothing above it.

use std::time::{Duration, Instant};

use crate::agent::{
    RuntimeState, SpeakerTurn, TEST_REACTION_COOLDOWN_S, TimingInput, candidate_lines, numbered,
    proactive_review, significant_change, silence_nudge, timing_decision, unreviewed_from,
};

/// How long the room has to be quiet before a pause is worth reading into.
///
/// Well under `SILENCE_THRESHOLD_S`, and that is the point: this is meant to
/// land in the ordinary gaps of an interview -- someone reading the problem,
/// typing, composing a sentence -- rather than in the stuck silences the
/// interviewer already steps into. It costs the candidate nothing either way,
/// because the call it starts runs beside the room instead of inside it.
pub(super) const INTERIM_IDLE: Duration = Duration::from_secs(8);
/// Between two idle-window reviews. A pause every eight seconds would spend a
/// call on every breath; this is roughly the interval at which an interview has
/// produced a new stretch worth reading.
pub(super) const INTERIM_COOLDOWN: Duration = Duration::from_secs(75);
/// What one review may read, in bytes.
///
/// The pause it runs in is the budget: `INTERIM_ATTEMPT_TIMEOUT` gives the call
/// twelve seconds, and a window too large to summarize in that spends its
/// prefill and returns nothing. Far below `MAX_TRANSCRIPT_BYTES`, which bounds
/// a whole interview rather than one stretch of it.
pub(super) const INTERIM_WINDOW_BYTES: usize = 8 * 1024;
/// What one review may read of the editor. Smaller than the transcript budget:
/// the code is re-sent in full on every review, where the transcript is only
/// the stretch since the last one.
pub(super) const INTERIM_CODE_BYTES: usize = 4 * 1024;
/// Candidate turns a pause has to have produced before one is read. Below this,
/// the window is an "mm-hm" and the note would be about nothing.
pub(super) const INTERIM_MIN_NEW_TURNS: usize = 4;

/// A candidate who has just typed is still working, even if their speech has
/// paused. Give them a beat before a periodic review tries to take the floor.
pub(super) const CODE_SETTLE: Duration = Duration::from_secs(10);

pub(super) struct RuntimeActivity {
    pub(super) last_code_change: Instant,
    pub(super) last_user_speech: Instant,
    /// When the candidate stopped talking and started waiting, if they have and
    /// the reply has not begun. Separate from `last_user_speech`, which the
    /// nudge logic needs seeded at the interview start and never absent.
    ///
    /// Sharing one field is what made the reply-latency line lie. `Interrupted`
    /// hands the floor back without ever saying when the candidate finished, so
    /// a reply that followed one measured from whatever the seed was: on the
    /// first turn that is the start of the interview, which is how a candidate
    /// who waited under a second was reported as having waited fifteen.
    ///
    /// Read as liveness as well as latency; see `reply_in_flight`.
    pub(super) awaiting_reply_since: Option<Instant>,
    pub(super) last_agent_speech: Instant,
    pub(super) last_nudge: Instant,
    pub(super) last_review: Instant,
    pub(super) last_interjection: Instant,
    pub(super) last_test_reaction: Instant,
    pub(super) code_at_last_review: String,
    pub(super) floor: Floor,
    /// A pause can arrive between Gemini producing a reply and this loop
    /// receiving its final event. Drop that old turn after resume too.
    pub(super) discarding_output: bool,
    /// When a pause was last read into. Sized against `INTERIM_COOLDOWN`.
    pub(super) last_interim: Instant,
    /// A tool response went out on this socket and its generation has not come
    /// back. Distinct from `awaiting_reply_since`, which a barge-in also stamps
    /// while Gemini owes nothing: this is generation already paid for, and
    /// replacing the transport under it throws the answer away on a socket
    /// nothing will ever read.
    pub(super) tool_response_outstanding: bool,
}

/// Whether a pause landing now leaves output still on its way.
///
/// Only a turn still being produced can deliver more events. Once it is
/// complete and merely draining, its `TurnComplete` has already been and gone,
/// so arming the discard on that state leaves it armed: nothing arrives to
/// disarm it, and the first reply after the resume is swallowed whole.
pub(super) fn pause_leaves_output_in_flight(floor: Floor) -> bool {
    matches!(floor, Floor::Speaking)
}

/// Who holds the conversation. `agent_busy` + `agent_turn_complete` encoded
/// this as two bools, but only three of the four combinations were reachable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Floor {
    /// The candidate has the floor.
    Listening,
    /// A prompt is in flight; Gemini is still producing the turn.
    Speaking,
    /// The turn is complete but queued audio is still draining.
    AwaitingPlayout,
}

impl RuntimeActivity {
    pub(super) fn new(now: Instant) -> Self {
        Self {
            last_code_change: now,
            last_user_speech: now,
            awaiting_reply_since: None,
            last_agent_speech: now,
            last_nudge: now,
            last_review: now,
            last_interjection: now,

            // Seeded in the past so the first test run reacts immediately; see
            // `CandidateMedia::new` for why the subtraction is checked.
            last_test_reaction: now
                .checked_sub(Duration::from_secs_f64(TEST_REACTION_COOLDOWN_S))
                .unwrap_or(now),
            code_at_last_review: String::new(),
            floor: Floor::Listening,
            discarding_output: false,
            tool_response_outstanding: false,

            // Seeded at `now` rather than in the past: the first minutes of an
            // interview are the greeting and the problem statement, and there
            // is nothing to assess in them.
            last_interim: now,
        }
    }

    /// The agent has the floor: it was just handed a prompt and owns the
    /// conversation until Gemini reports the turn complete.
    pub(super) fn mark_speaking(&mut self) {
        self.floor = Floor::Speaking;
    }

    /// Gemini owes a reply it has not begun to deliver.
    ///
    /// The second reader of `awaiting_reply_since`, and the reason its
    /// lifecycle is no longer only a metric's business: a `GoAway` weighs this
    /// before replacing the transport, because `Floor::Speaking` is stamped on
    /// the first audio chunk and so does not cover a turn that has issued a
    /// tool call and produced nothing yet. Tightening when the stamp is cleared
    /// now also changes when a socket is replaced.
    pub(super) fn reply_in_flight(&self) -> bool {
        self.awaiting_reply_since.is_some()
    }

    /// Turn finished and audio drained: hand the floor back to the candidate.
    pub(super) fn mark_listening(&mut self) {
        self.floor = Floor::Listening;
        self.last_agent_speech = Instant::now();
    }

    /// Gemini has transcribed something the candidate said, so they have
    /// stopped talking and started waiting.
    ///
    /// The two stamps are not the same fact, which is why they are separate
    /// fields. `last_user_speech` feeds the idle timers and has to move on
    /// every fragment. `awaiting_reply_since` is the start of a measurable
    /// wait, and it exists only while there is a wait to measure.
    ///
    /// Not armed while the agent holds the floor. Input transcription lags the
    /// audio it describes, so a fragment covering the tail of what the
    /// candidate said can land after the reply has already begun. Arming on
    /// that one made the next chunk of a turn already in progress announce
    /// itself as the reply starting, measured from a moment nobody waited from.
    pub(super) fn note_candidate_finished(&mut self, now: Instant) {
        self.last_user_speech = now;
        if self.floor != Floor::Speaking {
            self.awaiting_reply_since = Some(now);
        }
    }

    /// Whether this pause is worth spending an idle-window review on.
    ///
    /// Every condition here is "nothing is happening": nobody holds the floor,
    /// no reply or tool response is owed, the interview is neither paused nor
    /// over, and the candidate has been quiet long enough that a call started
    /// now will probably finish before they speak again. The last one is not
    /// about the room at all -- it asks whether anything has been said since
    /// the last review, because a pause in a silent stretch is not new
    /// evidence, it is the same silence.
    ///
    /// Nothing here blocks the interview. A `false` costs a comparison, and a
    /// `true` starts a call that runs beside the room loop; the interview is
    /// never waiting on either.
    ///
    /// Stamps its own cooldown on the way out, the way `watch_prompt` below
    /// applies the stamps its decision asks for. The caller doing it instead
    /// made this the one cooldown in the file kept somewhere other than where
    /// it is read, and left half the bookkeeping for a pause two functions from
    /// the other half. `ended` is not among the conditions: the only caller is
    /// a select arm already guarded on it, and `watch_prompt` does not re-ask
    /// it either.
    pub(super) fn claim_interim_review(&mut self, state: &RuntimeState, now: Instant) -> bool {
        let due = self.interim_review_due(state, now);
        if due {
            self.last_interim = now;
        }
        due
    }

    fn interim_review_due(&self, state: &RuntimeState, now: Instant) -> bool {
        !state.paused
            && self.floor == Floor::Listening
            && !self.reply_in_flight()
            && !self.tool_response_outstanding
            && now.duration_since(self.last_user_speech) >= INTERIM_IDLE
            && now.duration_since(self.last_interim) >= INTERIM_COOLDOWN
            && candidate_lines(&state.transcript[unreviewed_from(state)..]) >= INTERIM_MIN_NEW_TURNS
    }

    /// `now` is passed in rather than sampled here, like every other method on
    /// this struct. Sampling internally makes the boundaries untestable: a test
    /// can set `last_code_change` to exactly `CODE_SETTLE` ago, but the clock
    /// read inside would already have moved past it, so a half-open window and
    /// a closed one behave identically to every test that can be written.
    pub(super) fn watch_prompt(&mut self, state: &RuntimeState, now: Instant) -> Option<String> {
        if state.paused {
            return None;
        }
        let decision = timing_decision(&TimingInput {
            agent_busy: self.floor != Floor::Listening,
            user_talking: now.duration_since(self.last_code_change) < CODE_SETTLE,
            idle_seconds: now
                .duration_since(
                    self.last_user_speech
                        .max(self.last_code_change)
                        .max(self.last_agent_speech),
                )
                .as_secs_f64(),
            since_last_nudge_seconds: now.duration_since(self.last_nudge).as_secs_f64(),
            since_last_review_seconds: now.duration_since(self.last_review).as_secs_f64(),
            since_last_interjection_seconds: now
                .duration_since(self.last_interjection)
                .as_secs_f64(),
            speech_gap_seconds: now
                .duration_since(self.last_user_speech.max(self.last_agent_speech))
                .as_secs_f64(),
            significant_change: significant_change(&self.code_at_last_review, &state.code),
        });
        if decision.update_last_nudge {
            self.last_nudge = now;
        }
        if decision.update_last_review {
            self.last_review = now;
        }
        if decision.update_last_interjection {
            self.last_interjection = now;
        }
        if decision.sync_code_at_last_review {
            self.code_at_last_review = state.code.clone();
        }
        if decision.silence_nudge {
            return Some(silence_nudge(&numbered(&state.code)));
        }
        if decision.proactive_review {
            return Some(proactive_review(&numbered(&state.code)));
        }
        None
    }
}

/// What one interview accumulates, minus the two things the select loop borrows
/// as futures. `gemini` and `media` have to stay outside: their arms hold a
/// mutable borrow for as long as the select is polled, so bundling them here
/// would make every other arm's condition fight the borrow checker.
///
/// The four that are left exist to make [`TurnState::context`] possible. Three
/// arms of the select need a `GeminiEventContext`, and it borrows seven things
/// mutably, so it lives exactly as long as the call it is handed to. That used
/// to be a macro, purely to stop the field list being written out three times.
///
/// Worth knowing before anyone tries to go further: bundling these four and
/// stopping there makes the function *longer*, because every use site grows a
/// prefix and the macro stays. It only pays once the method exists to replace
/// the macro outright.
pub(super) struct TurnState {
    pub(super) state: RuntimeState,
    pub(super) agent_state: String,
    pub(super) activity: RuntimeActivity,
    pub(super) turns: SpeakerTurns,
}

/// Whether a candidate talking over the interviewer cuts it short.
///
/// Carried as an argument rather than a flag on the context. It was a field
/// that `send_wrap_up_and_wait` set and restored, which is one early return
/// away from leaving barge-in off for good, and it read as state when it is
/// really a property of the turn being handled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Interruptible {
    /// An ordinary exchange. The candidate speaking wins the floor.
    Yes,
    /// The closing message. Cutting it leaves the candidate without the ending,
    /// and the wrap-up wait reads the emptied queue as the turn being over, so
    /// a "thanks" mid sentence ended the interview there.
    No,
}

/// One open turn per speaker. Gemini interleaves input and output
/// transcription, so they cannot share a single accumulator.
#[derive(Debug, Default)]
pub(super) struct SpeakerTurns {
    pub(super) interviewer: SpeakerTurn,
    pub(super) candidate: SpeakerTurn,
}

pub(super) fn should_send_wrap_up(reason: &str) -> bool {
    reason != "candidate_ended"
}

#[cfg(test)]
#[path = "../../tests/unit/livekit/turn.rs"]
mod tests;
