//! The room's events as the loop reads them: each with the time it was
//! received, and the ones set aside while startup held the loop, handed out
//! again ahead of anything newer.

// This module reads the hub's imports and types through `use
// crate::livekit::*`.
#[cfg_attr(
    not(test),
    expect(
        clippy::wildcard_imports,
        reason = "the module reads its hub through crate::livekit::*"
    )
)]
use crate::livekit::*;

/// Receive time survives startup and ready publication; replay is not a new
/// departure or return, and cannot restart the candidate's presence grace.
pub(in crate::livekit) type TimedRoomEvent = (Instant, RoomEvent);

pub(in crate::livekit) struct RoomEventReader<Event = RoomEvent> {
    pub(in crate::livekit) events: tokio::sync::mpsc::UnboundedReceiver<Event>,
    pub(in crate::livekit) deferred: std::collections::VecDeque<(Instant, Event)>,
    /// The page asked for the ready notice while a socket replacement held
    /// the room and could not answer; the replacement's caller does.
    pub(in crate::livekit) startup_sync_pending: bool,
}

impl<Event> From<tokio::sync::mpsc::UnboundedReceiver<Event>> for RoomEventReader<Event> {
    fn from(events: tokio::sync::mpsc::UnboundedReceiver<Event>) -> Self {
        Self {
            events,
            deferred: Default::default(),
            startup_sync_pending: false,
        }
    }
}

impl<Event> RoomEventReader<Event> {
    /// Holds `events` to be handed out ahead of anything newer.
    pub(in crate::livekit) fn replay(&mut self, events: Vec<(Instant, Event)>) {
        self.deferred = events.into();
    }

    /// Records that the page asked for the ready notice while nothing could
    /// answer it.
    pub(in crate::livekit) fn ask_startup_sync(&mut self) {
        self.startup_sync_pending = true;
    }

    /// Whether the page asked since this was last called, and forgets it.
    pub(in crate::livekit) fn take_startup_sync_request(&mut self) -> bool {
        std::mem::take(&mut self.startup_sync_pending)
    }

    async fn recv_timed(&mut self) -> Option<(Instant, Event)> {
        match self.deferred.pop_front() {
            Some(event) => Some(event),
            None => self
                .events
                .recv()
                .await
                .map(|event| (tokio::time::Instant::now().into_std(), event)),
        }
    }

    pub(in crate::livekit) async fn recv(&mut self) -> Option<Event> {
        self.recv_timed().await.map(|(_, event)| event)
    }

    pub(in crate::livekit) fn try_recv(
        &mut self,
    ) -> Result<Event, tokio::sync::mpsc::error::TryRecvError> {
        match self.deferred.pop_front() {
            Some((_, event)) => Ok(event),
            None => self.events.try_recv(),
        }
    }
}

pub(in crate::livekit) async fn next_timed_room_event(
    events: &tokio::sync::Mutex<RoomEventReader>,
) -> Option<TimedRoomEvent> {
    events.lock().await.recv_timed().await
}

#[cfg(test)]
#[path = "../../../tests/unit/livekit/room/room_events.rs"]
mod tests;
