//! What the candidate's own work, as the data packets and the board report it,
//! does to the clocks the silence nudge and the reviews read.

use std::time::Instant;

use super::super::RuntimeActivity;
use crate::agent::DataEventResult;

impl RuntimeActivity {
    /// Stamps the clocks a data packet's reduction says it moved: the packet
    /// was the candidate's work, a test result the interviewer has now
    /// reacted to, or a word the interviewer has now said.
    pub(in crate::livekit) fn stamp_packet_effects(
        &mut self,
        result: &DataEventResult,
        at: Instant,
    ) {
        if result.update_last_code_change {
            self.last_code_change = at;
        }
        if result.update_last_test_reaction {
            self.last_test_reaction = at;
        }
        if result.update_last_interjection {
            self.last_interjection = at;
        }
    }

    /// How long ago the interviewer last reacted to a test result, which the
    /// reducer measures its reaction cooldown against.
    pub(in crate::livekit) fn test_reaction_age_secs(&self) -> f64 {
        self.last_test_reaction.elapsed().as_secs_f64()
    }

    /// A board snapshot arrived. The candidate is working, even while silent:
    /// without this the silence nudge counts a candidate who is drawing a
    /// diagram as idle and interrupts them mid-stroke, which is what
    /// `last_code_change` stops a typing candidate being asked. A snapshot
    /// that changed what the ledger holds also restarts the completed-coding
    /// idle count.
    pub(in crate::livekit) fn note_board_snapshot(&mut self, recorded: bool, now: Instant) {
        if recorded {
            self.reset_completed_coding_idle();
        }
        self.last_code_change = now;
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/livekit/turn/activity/candidate_work.rs"]
mod tests;
