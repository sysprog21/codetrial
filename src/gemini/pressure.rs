//! Local evidence of how full a socket's event channel got.
//!
//! The reader decodes frames and hands their events to the room loop through a
//! bounded channel. When the loop is slow the reader waits on that channel and
//! the socket goes unread, which looks from outside like a provider that went
//! quiet. These counters say which it was: they hold numbers only, never a
//! frame, a handle or a credential, and the reader updates them once per
//! event, so there is nothing per frame to log.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::time::Instant;

/// Why the reader task stopped, as far as it could say. A reader stopped from
/// outside (a replacement, a terminal exit, a failed write) never records one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ReaderEnd {
    /// Nothing was read for the idle limit. Time spent here is waiting on the
    /// provider, never counted as blocked on the channel.
    ReadIdle,
    /// A close frame, or the stream ended.
    Closed,
    /// The socket returned an error.
    Failed,
    /// The provider reported a credential failure.
    Credential,
    /// The room loop dropped its receiver.
    ReceiverClosed,
    /// Stopped from outside before it ended on its own.
    Stopped,
}

impl ReaderEnd {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::ReadIdle => "read_idle",
            Self::Closed => "closed",
            Self::Failed => "failed",
            Self::Credential => "credential",
            Self::ReceiverClosed => "receiver_closed",
            Self::Stopped => "stopped",
        }
    }
}

/// One socket's channel pressure, as reported once.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SocketPressure {
    /// Slots the channel holds.
    pub(crate) capacity: usize,
    /// The most events the channel held at once, never the current count.
    pub(crate) high_water: usize,
    /// Sends that found the channel full and had to wait.
    pub(crate) blocked_sends: u64,
    /// Time spent waiting in those sends. A send still waiting counts up to
    /// the moment this was taken.
    pub(crate) blocked_total: Duration,
    pub(crate) blocked_max: Duration,
    /// Events decoded from the socket, including one still waiting for a slot.
    pub(crate) received: u64,
    /// Events the room loop took from the channel.
    pub(crate) consumed: u64,
    pub(crate) end: ReaderEnd,
}

struct State {
    capacity: usize,
    high_water: usize,
    blocked_sends: u64,
    blocked_total: Duration,
    blocked_max: Duration,
    blocked_since: Option<Instant>,
    received: u64,
    consumed: u64,
    end: ReaderEnd,
    reported: bool,
}

/// Shared between the reader task, which counts what it receives and waits
/// for, and the session, which counts what the room loop consumes.
#[derive(Clone)]
pub(crate) struct Pressure(Arc<Mutex<State>>);

impl Pressure {
    pub(crate) fn new(capacity: usize) -> Self {
        Self(Arc::new(Mutex::new(State {
            capacity,
            high_water: 0,
            blocked_sends: 0,
            blocked_total: Duration::ZERO,
            blocked_max: Duration::ZERO,
            blocked_since: None,
            received: 0,
            consumed: 0,
            end: ReaderEnd::Stopped,
            reported: false,
        })))
    }

    /// The counters of a socket that was never connected, which has nothing to
    /// report.
    pub(crate) fn already_reported() -> Self {
        let pressure = Self::new(0);
        pressure.state().reported = true;
        pressure
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.0.lock().unwrap_or_else(|error| error.into_inner())
    }

    pub(crate) fn received(&self) {
        self.state().received += 1;
    }

    pub(crate) fn consumed(&self) {
        self.state().consumed += 1;
    }

    /// An event now sits in the channel, which holds `held` events.
    pub(crate) fn queued(&self, held: usize) {
        let mut state = self.state();
        state.high_water = state.high_water.max(held);
    }

    /// The send found the channel full. The wait ends when the guard drops,
    /// including when the reader is cancelled in the middle of it.
    pub(crate) fn block(&self) -> Blocked {
        let mut state = self.state();
        state.blocked_sends += 1;
        state.blocked_since = Some(Instant::now());
        Blocked(self.clone())
    }

    pub(crate) fn end(&self, end: ReaderEnd) {
        self.state().end = end;
    }

    /// The summary, once: the first caller gets it and every later one `None`,
    /// so a socket reported at its replacement is not reported again at exit.
    pub(crate) fn take_report(&self) -> Option<SocketPressure> {
        let mut state = self.state();
        if std::mem::replace(&mut state.reported, true) {
            return None;
        }
        Some(Self::snapshot(&state))
    }

    #[cfg(test)]
    pub(crate) fn peek(&self) -> SocketPressure {
        Self::snapshot(&self.state())
    }

    fn snapshot(state: &State) -> SocketPressure {
        let waiting = state.blocked_since.map(|since| since.elapsed());
        SocketPressure {
            capacity: state.capacity,
            high_water: state.high_water,
            blocked_sends: state.blocked_sends,
            blocked_total: state.blocked_total + waiting.unwrap_or_default(),
            blocked_max: state.blocked_max.max(waiting.unwrap_or_default()),
            received: state.received,
            consumed: state.consumed,
            end: state.end,
        }
    }
}

/// A send waiting for a slot.
pub(crate) struct Blocked(Pressure);

impl Drop for Blocked {
    fn drop(&mut self) {
        let mut state = self.0.state();
        if let Some(since) = state.blocked_since.take() {
            let waited = since.elapsed();
            state.blocked_total += waited;
            state.blocked_max = state.blocked_max.max(waited);
        }
    }
}
