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

impl Identity {
    fn from_view(view: &IdentityView<'_>) -> Self {
        Self {
            room: view.room.into(),
            session: view.session,
            socket: view.socket,
            trace_turn: view.trace_turn,
            clock: view.clock.clone(),
        }
    }
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
    /// Serialized JSON bytes, punctuation included, that each retained array
    /// (batches, receipts, replies and turn endings) is held to on its own, so
    /// a report can hold up to four times this. `calls` likewise bounds the
    /// receipts, the replies and the turn endings each.
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

/// A tool call as the handler received it, before the acknowledgement and
/// board awaits that can still cancel the batch. Received, executed and
/// locally sent are three separate facts, and none of them is the provider's
/// receipt.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub(in crate::livekit) struct Receipt {
    pub(in crate::livekit) identity: Identity,
    pub(in crate::livekit) id: String,
    pub(in crate::livekit) name: String,
    /// Delivered after a hold or discard had cut its generation off. Such a
    /// call is still answered, so it is kept, but it is never to be tied to a
    /// later reply.
    pub(in crate::livekit) discarded: bool,
    /// Sequence of the executed batch that answered it; `None` for a call that
    /// was received and never executed. Stands in for a stage: received,
    /// executed and locally sent are one lifecycle that the reply capture will
    /// fold into a single per-call record.
    pub(in crate::livekit) batch: Option<usize>,
}

/// The newest view of one interviewer transcript segment, as queued for the
/// page (the writer may coalesce waiting views, so `views` can exceed what the
/// page received). Only ordered facts are kept: how many receipts and batches
/// existed
/// when the segment first appeared says what could have preceded it, and
/// nothing here says what it was about.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub(in crate::livekit) struct Reply {
    /// Where the segment first appeared; later views keep it.
    pub(in crate::livekit) identity: Identity,
    pub(in crate::livekit) id: String,
    pub(in crate::livekit) text: String,
    pub(in crate::livekit) final_segment: bool,
    /// A view was not an extension of the one before it, or a final segment
    /// was published as open again: what the page showed was rewritten.
    pub(in crate::livekit) revised: bool,
    pub(in crate::livekit) views: usize,
    pub(in crate::livekit) receipts_before: usize,
    pub(in crate::livekit) batches_before: usize,
    /// The same counts at the newest view. A turn spans tool calls, so a
    /// segment whose two counts differ straddles them and says nothing about
    /// which call its later words follow.
    pub(in crate::livekit) receipts_last: usize,
    pub(in crate::livekit) batches_last: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize)]
pub(in crate::livekit) enum Ending {
    Complete,
    Interrupted,
}

/// A turn ending the provider sent, and whether the room loop took it. A
/// swallowed ending belongs to a generation already cut off, so it closes
/// nothing the page or the model saw; it is kept because the boundary a reply
/// would otherwise be bounded by is then missing. Local cuts and socket
/// replacements are not endings: they show as a sealed segment or a new socket
/// in the identity. An ending the old socket sent while a speech handoff
/// collected its events is consumed there and never seen here, so a socket
/// change between two replies is an unknown boundary, never an absent one.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub(in crate::livekit) struct Boundary {
    pub(in crate::livekit) identity: Identity,
    pub(in crate::livekit) ending: Ending,
    pub(in crate::livekit) accepted: bool,
    pub(in crate::livekit) receipts_before: usize,
    pub(in crate::livekit) batches_before: usize,
    pub(in crate::livekit) replies_before: usize,
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
    pub(in crate::livekit) receipts: Vec<Receipt>,
    /// Serialized length of the receipt array, brackets and commas included,
    /// held to the same byte limit as the batches but counted apart from them.
    pub(in crate::livekit) retained_receipt_bytes: usize,
    pub(in crate::livekit) replies: Vec<Reply>,
    pub(in crate::livekit) boundaries: Vec<Boundary>,
    /// Serialized length of each retained reply and of the two arrays,
    /// brackets and commas included, so a newer view of a reply, which
    /// arrives for every transcript chunk, is charged for its own change
    /// rather than by serializing every reply again under the lock.
    reply_bytes: Vec<usize>,
    retained_reply_bytes: usize,
    retained_boundary_bytes: usize,
}

impl Snapshot {
    /// The receipt a call executed under `identity` answers: same session and
    /// same socket, so a call received on one socket and executed on another
    /// is left unlinked as evidence of a replacement.
    fn receipt_mut(&mut self, identity: &IdentityView<'_>, id: &str) -> Option<&mut Receipt> {
        self.receipts.iter_mut().find(|receipt| {
            receipt.id == id
                && receipt.identity.session == identity.session
                && receipt.identity.socket == identity.socket
        })
    }
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
                receipts: Vec::new(),
                // The empty array, so the count is the serialized length.
                retained_receipt_bytes: 2,
                replies: Vec::new(),
                boundaries: Vec::new(),
                reply_bytes: Vec::new(),
                retained_reply_bytes: 2,
                retained_boundary_bytes: 2,
            }),
        }))
    }

    /// Whether the identity is the room and session this capture was admitted
    /// for; the socket may change under it.
    fn admits(&self, identity: &IdentityView<'_>) -> bool {
        identity.room == self.room && identity.session == self.session
    }

    /// Whether an array that serialized to `retained` bytes still fits once
    /// `value` is added to it, and its length if so.
    fn charged(&self, retained: usize, added: usize, value: &impl Serialize) -> Option<usize> {
        let mut counter = BoundedBytes {
            remaining: self.limits.bytes.checked_sub(retained + added)?,
            written: 0,
        };
        serde_json::to_writer(&mut counter, value).ok()?;
        Some(counter.written)
    }

    pub(in crate::livekit) fn snapshot(&self) -> Snapshot {
        self.inner
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Records the calls of one batch at the handler's entry. Scoped like a
    /// batch, bounded by the call and byte limits, and all or nothing: a batch
    /// that does not fit latches the failure and keeps none of its receipts.
    pub(in crate::livekit) fn receive(
        &self,
        identity: IdentityView<'_>,
        calls: &[GeminiFunctionCall],
        discarded: bool,
    ) {
        if calls.is_empty() {
            return;
        }
        let mut snapshot = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        if snapshot.failure.is_some() {
            return;
        }
        let failure = if !self.admits(&identity) {
            Some(Failure::Scope)
        } else if calls.len() > self.limits.calls.saturating_sub(snapshot.receipts.len()) {
            Some(Failure::Limit)
        } else if calls.iter().enumerate().any(|(index, call)| {
            call.id.is_empty()
                || calls[..index].iter().any(|earlier| earlier.id == call.id)
                || snapshot.receipts.iter().any(|old| old.id == call.id)
        }) {
            Some(Failure::AmbiguousId)
        } else {
            None
        };
        if let Some(failure) = failure {
            snapshot.failure = Some(failure);
            return;
        }
        let mut retained = snapshot.retained_receipt_bytes;
        let mut fresh = Vec::with_capacity(calls.len());
        for (index, call) in calls.iter().enumerate() {
            // A comma before every receipt but the very first.
            let comma = usize::from(snapshot.receipts.len() + index != 0);
            let receipt = Receipt {
                identity: Identity::from_view(&identity),
                id: call.id.clone(),
                name: call.name.clone(),
                discarded,
                batch: None,
            };
            let Some(bytes) = self.charged(retained, comma, &receipt) else {
                snapshot.failure = Some(Failure::Limit);
                return;
            };
            retained += comma + bytes;
            fresh.push(receipt);
        }
        snapshot.retained_receipt_bytes = retained;
        snapshot.receipts.extend(fresh);
    }

    /// Notes a published interviewer segment. A segment is identified by its
    /// session, socket and id, and only its newest text is kept, so the byte
    /// limit bounds the serialized array of newest views. A view that does not
    /// fit latches the failure and leaves the earlier ones as they were.
    pub(in crate::livekit) fn reply(
        &self,
        identity: IdentityView<'_>,
        id: &str,
        text: &str,
        final_segment: bool,
    ) {
        let mut snapshot = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        if snapshot.failure.is_some() {
            return;
        }
        if !self.admits(&identity) {
            snapshot.failure = Some(Failure::Scope);
            return;
        }
        let at = snapshot.replies.iter().position(|reply| {
            reply.id == id
                && reply.identity.session == identity.session
                && reply.identity.socket == identity.socket
        });
        let (reply, old) = if let Some(at) = at {
            // Built anew rather than cloned: the old text, the largest field,
            // is about to be replaced.
            let was = &snapshot.replies[at];
            let reply = Reply {
                identity: was.identity.clone(),
                id: was.id.clone(),
                text: text.into(),
                final_segment,
                revised: was.revised
                    || !text.starts_with(&was.text)
                    || (was.final_segment && !final_segment),
                views: was.views + 1,
                receipts_before: was.receipts_before,
                batches_before: was.batches_before,
                receipts_last: snapshot.receipts.len(),
                batches_last: snapshot.batches.len(),
            };
            (reply, snapshot.reply_bytes[at])
        } else if snapshot.replies.len() >= self.limits.calls {
            snapshot.failure = Some(Failure::Limit);
            return;
        } else {
            let reply = Reply {
                identity: Identity::from_view(&identity),
                id: id.into(),
                text: text.into(),
                final_segment,
                revised: false,
                views: 1,
                receipts_before: snapshot.receipts.len(),
                batches_before: snapshot.batches.len(),
                receipts_last: snapshot.receipts.len(),
                batches_last: snapshot.batches.len(),
            };
            (reply, 0)
        };
        // A comma before every reply but the first.
        let comma = usize::from(at.is_none() && !snapshot.replies.is_empty());
        let rest = snapshot.retained_reply_bytes - old;
        let Some(bytes) = self.charged(rest, comma, &reply) else {
            snapshot.failure = Some(Failure::Limit);
            return;
        };
        snapshot.retained_reply_bytes = rest + comma + bytes;
        if let Some(at) = at {
            snapshot.replies[at] = reply;
            snapshot.reply_bytes[at] = bytes;
        } else {
            snapshot.replies.push(reply);
            snapshot.reply_bytes.push(bytes);
        }
    }

    /// Notes a turn ending in arrival order. Bounded like the replies: by the
    /// call limit in count and the byte limit in serialized size.
    pub(in crate::livekit) fn boundary(
        &self,
        identity: IdentityView<'_>,
        ending: Ending,
        accepted: bool,
    ) {
        let mut snapshot = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        if snapshot.failure.is_some() {
            return;
        }
        if !self.admits(&identity) {
            snapshot.failure = Some(Failure::Scope);
            return;
        }
        let boundary = Boundary {
            identity: Identity::from_view(&identity),
            ending,
            accepted,
            receipts_before: snapshot.receipts.len(),
            batches_before: snapshot.batches.len(),
            replies_before: snapshot.replies.len(),
        };
        if snapshot.boundaries.len() >= self.limits.calls {
            snapshot.failure = Some(Failure::Limit);
            return;
        }
        let comma = usize::from(!snapshot.boundaries.is_empty());
        let Some(bytes) = self.charged(snapshot.retained_boundary_bytes, comma, &boundary) else {
            snapshot.failure = Some(Failure::Limit);
            return;
        };
        snapshot.retained_boundary_bytes += comma + bytes;
        snapshot.boundaries.push(boundary);
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
        let failure = if !self.admits(&identity) {
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
        let view = BatchView {
            identity: &identity,
            sequence,
            calls: CallsView(answers),
            local_send_completed: None,
        };
        let Some(bytes) = self.charged(snapshot.retained_bytes, comma, &view) else {
            snapshot.failure = Some(Failure::Limit);
            return None;
        };
        let batch = Batch {
            identity: Identity::from_view(&identity),
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
        snapshot.retained_bytes += comma + bytes;
        snapshot.retained_calls += answers.len();
        snapshot.batches.push(batch);

        // Linked only to a receipt from this same socket: a call received on
        // one socket and executed on another is evidence of a replacement, not
        // of an answer.
        let digits = sequence.to_string().len();
        let mut receipt_bytes = snapshot.retained_receipt_bytes;
        for (call, _) in answers {
            if let Some(receipt) = snapshot.receipt_mut(&identity, &call.id) {
                receipt.batch = Some(sequence);
                // `null` (four bytes) becomes the sequence number.
                receipt_bytes = (receipt_bytes + digits).saturating_sub(4);
            }
        }
        snapshot.retained_receipt_bytes = receipt_bytes;
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
                "receipts": snapshot.receipts,
                "retained_receipt_bytes": snapshot.retained_receipt_bytes,
                "replies": snapshot.replies,
                "boundaries": snapshot.boundaries,
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

fn active() -> Option<Arc<Capture>> {
    ACTIVE
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .as_ref()
        .map(Arc::clone)
}

pub(in crate::livekit) fn observe(
    latency: &ReplyLatency,
    answers: &[(GeminiFunctionCall, Value)],
) -> Option<Ticket> {
    let capture = active()?;
    let sequence = capture.record(latency.acquisition_identity(), answers)?;
    Some(Ticket { capture, sequence })
}

/// The handler's entry, before anything that can cancel the batch.
pub(in crate::livekit) fn receive(
    latency: &ReplyLatency,
    calls: &[GeminiFunctionCall],
    discarded: bool,
) {
    if let Some(capture) = active() {
        capture.receive(latency.acquisition_identity(), calls, discarded);
    }
}

/// A turn ending as the room loop decided it; other events are not endings.
pub(in crate::livekit) fn boundary(
    latency: &ReplyLatency,
    event: &crate::gemini::GeminiEvent,
    accepted: bool,
) {
    use crate::gemini::GeminiEvent;
    let ending = match event {
        GeminiEvent::TurnComplete => Ending::Complete,
        GeminiEvent::Interrupted => Ending::Interrupted,
        _ => return,
    };
    if let Some(capture) = active() {
        capture.boundary(latency.acquisition_identity(), ending, accepted);
    }
}

/// An interviewer segment as the room loop publishes it.
pub(in crate::livekit) fn reply(latency: &ReplyLatency, id: &str, text: &str, final_segment: bool) {
    if let Some(capture) = active() {
        capture.reply(latency.acquisition_identity(), id, text, final_segment);
    }
}

#[path = "../../../tests/unit/livekit/probe/acquisition.rs"]
mod tests;
