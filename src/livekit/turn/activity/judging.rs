//! The round transition held for the phase judge.

use std::time::{Duration, Instant};

use super::super::{ParkedPacket, RuntimeActivity};
use crate::agent::RuntimeState;

impl RuntimeActivity {
    /// Holds a round-transition packet for the phase judge, until the judgment
    /// settles or `settle` has passed since `now`. A repeat of the same
    /// request keeps the first one's hold, and so its deadline.
    pub(in crate::livekit) fn hold_round_transition(
        &mut self,
        payload: &serde_json::Value,
        received: bool,
        now: Instant,
        settle: Duration,
    ) {
        self.pending_round_transition
            .get_or_insert_with(|| ParkedPacket {
                payload: payload.clone(),
                received,
                deadline: now + settle,
            });
    }

    /// The held transition, once it is ready to be applied; see
    /// `parked_transition_ready`.
    pub(in crate::livekit) fn take_ready_parked_transition(
        &mut self,
        state: &RuntimeState,
        now: Instant,
    ) -> Option<ParkedPacket> {
        if self.parked_transition_ready(state, now) {
            self.pending_round_transition.take()
        } else {
            None
        }
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/livekit/turn/activity/judging.rs"]
mod tests;
