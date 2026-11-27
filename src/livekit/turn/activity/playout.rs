//! What the interviewer's voice reaching the queue does to the activity.

use std::time::Instant;

use super::super::{RotationWatch, RuntimeActivity};
use crate::livekit::LiveOutcome;

impl RuntimeActivity {
    /// Reply audio was queued and will finish playing at `playout_ends_at`.
    /// Returns the replacement watch this first audio settles, if one was
    /// armed.
    ///
    /// Speech is queued, not played: the floor stays busy until the buffered
    /// audio actually finishes.
    pub(in crate::livekit) fn note_reply_queued(
        &mut self,
        playout_ends_at: Instant,
    ) -> Option<RotationWatch> {
        self.note_reply_audible();
        let watch = self.rotation.take();
        self.last_agent_speech = playout_ends_at;
        watch
    }

    /// This session's usage summary, as the log analyzer reads it.
    pub(in crate::livekit) fn live_usage_line(
        &self,
        room: &str,
        model: &str,
        elapsed_s: u64,
        outcome: LiveOutcome,
    ) -> String {
        crate::livekit::live_usage_line(
            room,
            self.live_session_id,
            model,
            elapsed_s,
            outcome,
            Some(self.live_socket),
            &self.live_usage,
        )
    }
}

#[cfg(test)]
impl RuntimeActivity {
    /// What a test reads to see whether a replacement's first audio is still
    /// being waited for.
    pub(in crate::livekit) fn take_rotation_watch(&mut self) -> Option<RotationWatch> {
        self.rotation.take()
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/livekit/turn/activity/playout.rs"]
mod tests;
