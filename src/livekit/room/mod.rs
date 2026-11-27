//! The LiveKit room as the interview reaches it: who joins and when, the events
//! it
//! delivers, the link the handlers send through, the management API and the
//! rooms an
//! agent has to leave.

pub(in crate::livekit) mod candidate;
pub(in crate::livekit) mod room_events;
pub(in crate::livekit) mod room_link;
pub(in crate::livekit) mod room_service;
pub(in crate::livekit) mod rooms;
