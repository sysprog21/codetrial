//! The two observers that watch an interview's turns without deciding
//! anything: the per-reply latency trace and the turn-timing summary.
//!
//! They sit beside [`super::RuntimeActivity`], not inside it. The activity
//! holds what the interviewer owes and when it last spoke; nothing it does
//! reads these, so a trace can be added or dropped without touching the
//! floor, stall or nudge rules.

use super::latency::ReplyLatency;
use super::timing::TurnTiming;

#[derive(Default)]
pub(in crate::livekit) struct TurnProbes {
    pub(in crate::livekit) latency: ReplyLatency,
    pub(in crate::livekit) turn_timing: TurnTiming,
}
