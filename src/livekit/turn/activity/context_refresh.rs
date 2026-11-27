//! The compressed-context checkpoint and the token accounting behind it.

use std::time::Instant;

use super::super::RuntimeActivity;
use crate::agent::RuntimeState;

impl RuntimeActivity {
    /// Whether a compressed-context checkpoint is due. Runs after every Gemini
    /// event, most of them audio, and nothing pending is the common case, so
    /// `gates` (output playing, the candidate's turn open) is read only once
    /// something is pending: the second reads the candidate's open turn to say
    /// no.
    pub(in crate::livekit) fn context_checkpoint_due(
        &self,
        state: &RuntimeState,
        now: Instant,
        gates: impl FnOnce() -> (bool, bool),
    ) -> bool {
        if !self.tokens.context_refresh_needed() {
            return false;
        }
        let (audio_playing, candidate_open) = gates();
        self.checkpoint_due(state, audio_playing, candidate_open, now)
    }

    /// The checkpoint reached the model: nothing is pending, and whether the
    /// provider may cut the context is read again.
    pub(in crate::livekit) fn context_refreshed(&mut self, state: &mut RuntimeState) {
        self.tokens.context_refreshed();
        self.sync_cut_plausible(state);
    }

    /// Every completion ends a generation the provider billed and whose context
    /// it may have cut, including the late one an interruption or a hold leaves
    /// behind.
    pub(in crate::livekit) fn observe_turn_complete(&mut self, state: &mut RuntimeState) {
        self.tokens.observe_turn_complete(state.context_compression);
        self.sync_cut_plausible(state);
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/livekit/turn/activity/context_refresh.rs"]
mod tests;
