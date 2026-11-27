//! What cutting a turn short leaves behind on the activity.

use std::time::Instant;

use super::super::{Floor, PROMPT_STALL, RuntimeActivity};

impl RuntimeActivity {
    /// The turn was cut: the pending measurement, the wait on the prompt and
    /// the generation all die with it.
    ///
    /// A candidate who finished, waited, and then started talking again is no
    /// longer waiting for anything, and leaving the stamp behind meant the next
    /// reply that produced audio without its own transcript measured from it:
    /// the same lie `awaiting_reply_since` was split out of `last_user_speech`
    /// to stop telling, one turn later. A tool continuation the generation was
    /// waiting for died with it too.
    pub(in crate::livekit) fn end_cut_turn(&mut self) {
        self.clear_candidate_reply_wait();
        self.clear_prompt_wait();
        self.prompt_behind_turn = false;
        self.generating = false;
        self.last_output_at = None;
        self.socket.tool_response_outstanding = false;
    }

    /// A hold took the floor: arm the discard of the turn still in flight, or
    /// keep the late ending of a stalled one from settling the release's
    /// prompt. A discard already under way is kept.
    pub(in crate::livekit) fn arm_hold_discard(&mut self, now: Instant) {
        self.socket.discarding_output |= self.hold_leaves_output_in_flight(now);
        self.socket.stale_turn_pending |= self.generating && !self.socket.discarding_output;
    }

    /// Whether a hold landing now leaves output still on its way.
    ///
    /// Only a turn still being produced can deliver more events. Once it is
    /// complete and merely draining, its `TurnComplete` has already been and
    /// gone,
    /// so arming the discard on that state leaves it armed: nothing arrives to
    /// disarm it, and the first reply after the release is swallowed whole.
    ///
    /// A generation or tool continuation that has produced nothing for
    /// `PROMPT_STALL` is not on its way either, the way `settle_stalls` stops
    /// counting a prompt then. A live one ends when the release prompt reaches
    /// Gemini, which interrupts it and so ends the discard; a dead one never
    /// ends,
    /// and a discard armed on it dropped the answer to the release prompt until
    /// the
    /// watchdog asked again. That covers the floor too: audio a stalled
    /// generation
    /// already delivered took `Floor::Speaking`, and only its `TurnComplete`
    /// would
    /// have handed the floor back.
    pub(in crate::livekit) fn hold_leaves_output_in_flight(&self, now: Instant) -> bool {
        let continuation_live = self.socket.tool_response_outstanding
            && self
                .tool_response_at
                .is_some_and(|at| now.saturating_duration_since(at) < PROMPT_STALL);
        ((self.floor == Floor::Speaking || self.generating) && !self.generation_stalled(now))
            || continuation_live
    }
}
