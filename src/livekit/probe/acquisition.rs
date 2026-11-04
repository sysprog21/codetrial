//! Bounded private execution evidence for authored acquisition fixtures.

pub(in crate::livekit) mod owner;

use std::io::{self, Write};
use std::sync::{Arc, Mutex, PoisonError};

use serde::ser::{Serialize, SerializeSeq, Serializer};
use serde_json::Value;

use crate::gemini::GeminiFunctionCall;
use crate::livekit::latency::ReplyLatency;

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub(in crate::livekit) struct Identity {
    pub(in crate::livekit) room: String,
    pub(in crate::livekit) session: u64,
    pub(in crate::livekit) socket: u64,
    pub(in crate::livekit) trace_turn: Option<u64>,
    pub(in crate::livekit) clock: Value,
}

#[derive(serde::Serialize)]
pub(in crate::livekit) struct IdentityView<'a> {
    pub(in crate::livekit) room: &'a str,
    pub(in crate::livekit) session: u64,
    pub(in crate::livekit) socket: u64,
    pub(in crate::livekit) trace_turn: Option<u64>,
    pub(in crate::livekit) clock: Value,
}

#[derive(Clone, Copy)]
pub(in crate::livekit) struct Limits {
    pub(in crate::livekit) batches: usize,
    pub(in crate::livekit) calls: usize,
    /// Serialized JSON bytes of the retained batch array, including
    /// punctuation.
    pub(in crate::livekit) bytes: usize,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub(in crate::livekit) struct Call {
    pub(in crate::livekit) id: String,
    pub(in crate::livekit) name: String,
    pub(in crate::livekit) result: Value,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub(in crate::livekit) struct Batch {
    pub(in crate::livekit) identity: Identity,
    pub(in crate::livekit) sequence: usize,
    pub(in crate::livekit) calls: Vec<Call>,
    /// None means no local send completion was observed, not a failed delivery.
    pub(in crate::livekit) local_send_completed: Option<bool>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::livekit) enum Failure {
    // Room/session admission or an opaque ticket's batch association differs.
    Scope,
    Limit,
    // Missing or reused provider IDs cannot be correlated safely.
    AmbiguousId,
}

#[derive(Clone, Debug, PartialEq)]
pub(in crate::livekit) struct Snapshot {
    pub(in crate::livekit) batches: Vec<Batch>,
    pub(in crate::livekit) failure: Option<Failure>,
    pub(in crate::livekit) retained_calls: usize,
    pub(in crate::livekit) retained_bytes: usize,
}

pub(in crate::livekit) struct Capture {
    room: String,
    session: u64,
    limits: Limits,
    inner: Mutex<Snapshot>,
}

impl Capture {
    pub(in crate::livekit) fn new(
        room: &str,
        session: u64,
        limits: Limits,
    ) -> Result<Arc<Self>, &'static str> {
        if room.is_empty() || session == 0 {
            return Err("acquisition requires an assigned room and nonzero session");
        }
        if limits.batches == 0 || limits.calls == 0 || limits.bytes < 2 {
            return Err("acquisition requires positive retention limits");
        }

        // Bound the admitted scope before retaining its room string. Full batch
        // accounting still happens at record time, including its actual clock.
        let scope = IdentityView {
            room,
            session,
            socket: 0,
            trace_turn: None,
            clock: Value::Null,
        };
        let mut counter = BoundedBytes {
            remaining: limits.bytes - 2,
            written: 0,
        };
        if serde_json::to_writer(&mut counter, &scope).is_err() {
            return Err("acquisition scope metadata exceeds byte limit");
        }
        Ok(Arc::new(Self {
            room: room.into(),
            session,
            limits,
            inner: Mutex::new(Snapshot {
                batches: Vec::new(),
                failure: None,
                retained_calls: 0,
                retained_bytes: 2,
            }),
        }))
    }

    pub(in crate::livekit) fn snapshot(&self) -> Snapshot {
        self.inner
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn record(
        &self,
        identity: IdentityView<'_>,
        answers: &[(GeminiFunctionCall, Value)],
    ) -> Option<usize> {
        if answers.is_empty() {
            return None;
        }
        let mut snapshot = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        if snapshot.failure.is_some() {
            return None;
        }
        let failure = if identity.room != self.room || identity.session != self.session {
            Some(Failure::Scope)
        } else if snapshot.batches.len() >= self.limits.batches
            || answers.len() > self.limits.calls.saturating_sub(snapshot.retained_calls)
        {
            Some(Failure::Limit)
        } else if answers.iter().enumerate().any(|(index, (call, _))| {
            call.id.is_empty()
                || answers[..index]
                    .iter()
                    .any(|(earlier, _)| earlier.id == call.id)
                || snapshot
                    .batches
                    .iter()
                    .any(|batch| batch.calls.iter().any(|old| old.id == call.id))
        }) {
            Some(Failure::AmbiguousId)
        } else {
            None
        };
        if let Some(failure) = failure {
            snapshot.failure = Some(failure);
            return None;
        }
        let sequence = snapshot.batches.len();
        let comma = usize::from(sequence != 0);
        let remaining = self.limits.bytes.saturating_sub(snapshot.retained_bytes);
        let mut counter = BoundedBytes {
            remaining: remaining.saturating_sub(comma),
            written: 0,
        };
        let view = BatchView {
            identity: &identity,
            sequence,
            calls: CallsView(answers),
            local_send_completed: None,
        };
        if remaining < comma || serde_json::to_writer(&mut counter, &view).is_err() {
            snapshot.failure = Some(Failure::Limit);
            return None;
        }
        let batch = Batch {
            identity: Identity {
                room: identity.room.into(),
                session: identity.session,
                socket: identity.socket,
                trace_turn: identity.trace_turn,
                clock: identity.clock,
            },
            sequence,
            calls: answers
                .iter()
                .map(|(call, result)| Call {
                    id: call.id.clone(),
                    name: call.name.clone(),
                    result: result.clone(),
                })
                .collect(),
            local_send_completed: None,
        };
        snapshot.retained_bytes += counter.written + comma;
        snapshot.retained_calls += answers.len();
        snapshot.batches.push(batch);
        Some(sequence)
    }
}

/// Admission fixes the batch's identity permanently: batches only append.
/// An original capture and its sequence associate completion without copying
/// another clock object or looking up whichever capture is installed later.
pub(in crate::livekit) struct Ticket {
    capture: Arc<Capture>,
    sequence: usize,
}

impl Ticket {
    pub(in crate::livekit) fn complete(self, sent: bool) {
        let mut snapshot = self
            .capture
            .inner
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if snapshot.failure.is_some() {
            return;
        }
        if snapshot
            .batches
            .get(self.sequence)
            .is_none_or(|batch| batch.local_send_completed.is_some())
        {
            snapshot.failure = Some(Failure::Scope);
            return;
        }
        let extra = usize::from(!sent);
        if extra
            > self
                .capture
                .limits
                .bytes
                .saturating_sub(snapshot.retained_bytes)
        {
            snapshot.failure = Some(Failure::Limit);
            return;
        }
        // null and true have equal JSON width; false needs one more byte.
        snapshot.batches[self.sequence].local_send_completed = Some(sent);
        snapshot.retained_bytes += extra;
    }
}

#[derive(serde::Serialize)]
struct BatchView<'a> {
    identity: &'a IdentityView<'a>,
    sequence: usize,
    calls: CallsView<'a>,
    local_send_completed: Option<bool>,
}

#[derive(serde::Serialize)]
struct CallView<'a> {
    id: &'a str,
    name: &'a str,
    result: &'a Value,
}

struct CallsView<'a>(&'a [(GeminiFunctionCall, Value)]);
impl Serialize for CallsView<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut sequence = serializer.serialize_seq(Some(self.0.len()))?;
        for (call, result) in self.0 {
            sequence.serialize_element(&CallView {
                id: &call.id,
                name: &call.name,
                result,
            })?;
        }
        sequence.end()
    }
}

struct BoundedBytes {
    remaining: usize,
    written: usize,
}
impl Write for BoundedBytes {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > self.remaining {
            return Err(io::Error::other("acquisition byte limit"));
        }
        self.remaining -= bytes.len();
        self.written += bytes.len();
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Why an arming observer stopped trusting its scope. Latched: a fixture that
/// saw any of these has no evidence to report.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::livekit) enum ArmFailure {
    /// A bind arrived before any assignment named a room.
    BindBeforeAssignment,
    /// A second, different room was assigned or bound.
    UnrelatedRoom,
    /// A bind carried session zero, or a different session than the first.
    Session,
    /// The bound scope could not be admitted by `Capture::new`.
    Scope,
    /// Another capture already held the single installation slot.
    Install,
}

/// Dispatch-to-bind handoff for a fixture that cannot know the room name or
/// session id in advance. It starts unarmed and admits nothing:
///
/// - the dispatcher's accepted assignment names the one expected room;
/// - the first bind for that room latches its nonzero session, builds the
///   `Capture` and installs it, so a tool batch can never arrive at an unbound
///   scope;
/// - a bind before any assignment, a second room, session zero, a different
///   later session or an occupied installation slot fails the observer for
///   good, which also uninstalls the capture: evidence from a scope it doubted
///   would mislead.
///
/// The observer owns the install guard, so removing the observer (`retire`)
/// or dropping it takes the capture out with it, and a callback that was
/// already in flight finds a retired observer and does nothing.
pub(in crate::livekit) struct Arming {
    limits: Limits,
    /// Whether a bound scope is installed for `observe` to find. Off only for
    /// the detached form, which exists so the state machine can be tested
    /// without touching the process-wide slot.
    publish: bool,
    state: Mutex<ArmState>,
}

enum ArmState {
    Unarmed,
    Expecting(String),
    Bound {
        capture: Arc<Capture>,
        _installed: Option<Installed>,
    },
    Failed(ArmFailure),
    Retired,
}

impl ArmState {
    fn room(&self) -> Option<&str> {
        match self {
            Self::Expecting(room) => Some(room),
            Self::Bound { capture, .. } => Some(&capture.room),
            _ => None,
        }
    }

    fn is_terminal(&self) -> bool {
        matches!(self, Self::Failed(_) | Self::Retired)
    }
}

impl Arming {
    pub(in crate::livekit) fn new(limits: Limits) -> Arc<Self> {
        Self::build(limits, true)
    }

    /// The same rules without installing anything.
    pub(in crate::livekit) fn detached(limits: Limits) -> Arc<Self> {
        Self::build(limits, false)
    }

    fn build(limits: Limits, publish: bool) -> Arc<Self> {
        Arc::new(Self {
            limits,
            publish,
            state: Mutex::new(ArmState::Unarmed),
        })
    }

    fn state(&self) -> std::sync::MutexGuard<'_, ArmState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(in crate::livekit) fn on_assignment(&self, room: &str) {
        let mut state = self.state();
        if state.is_terminal() {
            return;
        }
        match state.room() {
            None => *state = ArmState::Expecting(room.into()),
            Some(expected) if expected == room => {}
            Some(_) => *state = ArmState::Failed(ArmFailure::UnrelatedRoom),
        }
    }

    pub(in crate::livekit) fn on_bind(&self, room: &str, session: u64) {
        let mut state = self.state();
        if state.is_terminal() {
            return;
        }
        if let Some(next) = self.after_bind(&state, room, session) {
            *state = next;
        }
    }

    /// The state a bind moves to, or `None` when it changes nothing.
    fn after_bind(&self, state: &ArmState, room: &str, session: u64) -> Option<ArmState> {
        let fail = |failure| Some(ArmState::Failed(failure));
        match state {
            ArmState::Unarmed => fail(ArmFailure::BindBeforeAssignment),
            _ if state.room() != Some(room) => fail(ArmFailure::UnrelatedRoom),
            ArmState::Bound { capture, .. } if capture.session != session => {
                fail(ArmFailure::Session)
            }
            ArmState::Expecting(_) if session == 0 => fail(ArmFailure::Session),
            ArmState::Expecting(_) => {
                let Ok(capture) = Capture::new(room, session, self.limits) else {
                    return fail(ArmFailure::Scope);
                };
                let installed = if self.publish {
                    match install(&capture) {
                        Ok(installed) => Some(installed),
                        Err(_) => return fail(ArmFailure::Install),
                    }
                } else {
                    None
                };
                Some(ArmState::Bound {
                    capture,
                    _installed: installed,
                })
            }
            _ => None,
        }
    }

    /// Ends the observer: whatever it installed is removed and later events
    /// are ignored.
    fn retire(&self) {
        *self.state() = ArmState::Retired;
    }

    /// The admitted scope, once bound and for as long as nothing has failed.
    pub(in crate::livekit) fn capture(&self) -> Option<Arc<Capture>> {
        match &*self.state() {
            ArmState::Bound { capture, .. } => Some(Arc::clone(capture)),
            _ => None,
        }
    }

    /// What this observer saw, for a fixture to leave behind: the failure that
    /// ended its trust, if any, and the admitted scope's retained evidence.
    pub(in crate::livekit) fn report(&self) -> serde_json::Value {
        let capture = self.capture().map(|capture| {
            let snapshot = capture.snapshot();
            serde_json::json!({
                "batches": snapshot.batches,
                "failure": snapshot.failure.map(|failure| format!("{failure:?}")),
                "retained_calls": snapshot.retained_calls,
                "retained_bytes": snapshot.retained_bytes,
            })
        });
        serde_json::json!({
            "arming_failure": self.failure().map(|failure| format!("{failure:?}")),
            "capture": capture,
        })
    }

    pub(in crate::livekit) fn failure(&self) -> Option<ArmFailure> {
        match &*self.state() {
            ArmState::Failed(failure) => Some(*failure),
            _ => None,
        }
    }
}

static ARMING: Mutex<Option<Arc<Arming>>> = Mutex::new(None);
static ACTIVE: Mutex<Option<Arc<Capture>>> = Mutex::new(None);

/// Uninstall stops new lookups, but admitted tickets can still complete.
/// Fixtures must join admitted writers before taking a final snapshot.
pub(in crate::livekit) struct Installed(Arc<Capture>);
impl Drop for Installed {
    fn drop(&mut self) {
        let mut active = ACTIVE.lock().unwrap_or_else(PoisonError::into_inner);
        if active.as_ref().is_some_and(|now| Arc::ptr_eq(now, &self.0)) {
            *active = None;
        }
    }
}

pub(in crate::livekit) fn install(capture: &Arc<Capture>) -> Result<Installed, &'static str> {
    let mut active = ACTIVE.lock().unwrap_or_else(PoisonError::into_inner);
    if active.is_some() {
        return Err("an acquisition capture is already installed");
    }
    *active = Some(Arc::clone(capture));
    Ok(Installed(Arc::clone(capture)))
}

/// Removes the arming observer and anything it installed when dropped.
pub(in crate::livekit) struct ArmingInstalled {
    arming: Arc<Arming>,
    hook: crate::dispatch::AssignmentHook,
}
impl Drop for ArmingInstalled {
    fn drop(&mut self) {
        // One critical section, the same lock `install_arming` takes: a
        // replacement observer can only install once this one is entirely gone,
        // so this teardown never clears the replacement's hook or meets this
        // observer's capture still installed.
        let mut arming = ARMING.lock().unwrap_or_else(PoisonError::into_inner);
        if arming
            .as_ref()
            .is_some_and(|now| Arc::ptr_eq(now, &self.arming))
        {
            *arming = None;
        }
        crate::dispatch::clear_assignment_hook(&self.hook);
        self.arming.retire();
    }
}

/// Starts watching the dispatcher's assignments and the latency binds for the
/// one room the fixture will be given. Nothing is captured until both agree.
pub(in crate::livekit) fn install_arming(
    arming: &Arc<Arming>,
) -> Result<ArmingInstalled, &'static str> {
    let mut slot = ARMING.lock().unwrap_or_else(PoisonError::into_inner);
    if slot.is_some() {
        return Err("an acquisition arming observer is already installed");
    }
    *slot = Some(Arc::clone(arming));
    let watching = Arc::clone(arming);
    let hook: crate::dispatch::AssignmentHook = Arc::new(move |room: &str| {
        watching.on_assignment(room);
    });
    crate::dispatch::set_assignment_hook(Some(Arc::clone(&hook)));
    Ok(ArmingInstalled {
        arming: Arc::clone(arming),
        hook,
    })
}

/// The latency bind boundary: called with every room and session the loop
/// binds.
pub(in crate::livekit) fn bound(room: &str, session: u64) {
    let arming = ARMING
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    if let Some(arming) = arming {
        arming.on_bind(room, session);
    }
}

pub(in crate::livekit) fn observe(
    latency: &ReplyLatency,
    answers: &[(GeminiFunctionCall, Value)],
) -> Option<Ticket> {
    let capture = ACTIVE
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .as_ref()
        .map(Arc::clone)?;
    let sequence = capture.record(latency.acquisition_identity(), answers)?;
    Some(Ticket { capture, sequence })
}

#[path = "../../../tests/unit/livekit/probe/acquisition.rs"]
mod tests;
