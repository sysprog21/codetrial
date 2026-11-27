//! What events do to a [`super::RuntimeActivity`], one intent per module, so
//! that code outside the type asks for the effect instead of writing the
//! fields it is made of.

mod candidate_work;
mod context_refresh;
mod cuts;
mod events;
mod handover;
mod judging;
mod playout;

pub(in crate::livekit) use events::{
    OutputDisposition, TurnBoundary, answers_prompt, ends_turn, is_output, output_disposition,
    turn_boundary,
};
