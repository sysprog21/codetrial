//! Observation points for the credentialed speech-continuity fixture. Compiled
//! only for tests: there is no production fault flag and nothing here changes
//! what a release build does.
//!
//! A fixture that hosts the real room loop installs one `ContinuityProbe`. The
//! loop then reports to it through three seams:
//!
//! * the microphone dequeue, where the probe's `DequeueGate` can hold the
//!   consumer while every other arm of the loop keeps running,
//! * each Gemini input transcript fragment, as the loop received it,
//! * the loop's own audio counters (ingress drops and quarantined bytes).
//!
//! The gate holds only the one future that would take the next frame from the
//! SDK queue. It sits in front of the dequeue, so a future cancelled by another
//! arm of the loop's `select!` has taken nothing, and nothing here suspends the
//! runtime or the provider's clock.
//!
//! A hold takes effect at the next dequeue the consumer begins. A consumer
//! already parked on the SDK stream when the gate closes can still take the one
//! frame that arrives next, so the hold starts within about one frame (10 ms)
//! of being asked for.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use futures_util::{Stream, StreamExt};

/// What the gate saw, kept small: a per-second tally rather than a row per
/// ten-millisecond frame.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct DequeueStats {
    /// Times the consumer reached the gate while it was held.
    pub(super) held_arrivals: u64,
    /// When the gate was last closed and last opened.
    pub(super) held_at: Option<Instant>,
    pub(super) released_at: Option<Instant>,
    /// Frames taken from the SDK queue, and when the first and last were.
    pub(super) frames: u64,
    pub(super) first_frame_at: Option<Instant>,
    pub(super) last_frame_at: Option<Instant>,
    /// The first frame taken after the last release.
    pub(super) first_after_release_at: Option<Instant>,
    pub(super) frames_after_release: u64,
    /// Frames taken in each whole second since the gate's epoch; a second
    /// before the epoch is negative.
    pub(super) per_second: BTreeMap<i64, u64>,
}

pub(super) struct DequeueGate {
    held: tokio::sync::watch::Sender<bool>,
    epoch: Mutex<Instant>,
    stats: Mutex<DequeueStats>,
}

impl DequeueGate {
    pub(super) fn new(epoch: Instant) -> Self {
        Self {
            held: tokio::sync::watch::channel(false).0,
            epoch: Mutex::new(epoch),
            stats: Mutex::new(DequeueStats::default()),
        }
    }

    /// Holds the consumer at its next dequeue. Idempotent.
    pub(super) fn hold(&self) {
        if !self.held.send_replace(true) {
            let mut stats = self.stats.lock().unwrap();
            stats.held_at = Some(Instant::now());
        }
    }

    /// Lets the consumer through. Idempotent.
    pub(super) fn release(&self) {
        if self.held.send_replace(false) {
            let mut stats = self.stats.lock().unwrap();
            stats.released_at = Some(Instant::now());
            stats.first_after_release_at = None;
            stats.frames_after_release = 0;
        }
    }

    /// Counts the per-second tally from `epoch` from now on, and forgets the
    /// tally so far: the fixture's arm clock starts after the interview does.
    pub(super) fn rebase(&self, epoch: Instant) {
        *self.epoch.lock().unwrap() = epoch;
        self.stats.lock().unwrap().per_second.clear();
    }

    pub(super) fn is_held(&self) -> bool {
        *self.held.borrow()
    }

    pub(super) fn stats(&self) -> DequeueStats {
        self.stats.lock().unwrap().clone()
    }

    /// Returns once the gate is open. Only this future waits.
    async fn wait_open(&self) {
        let mut open = self.held.subscribe();
        let mut counted = false;
        loop {
            if !*open.borrow_and_update() {
                return;
            }
            if !counted {
                counted = true;
                self.stats.lock().unwrap().held_arrivals += 1;
            }
            if open.changed().await.is_err() {
                return;
            }
        }
    }

    /// Notes one frame taken from the queue. Called only after the frame is in
    /// hand, which is after the gate: a time stamped before it would be the
    /// moment the consumer arrived, not the moment the frame went on.
    fn record_frame(&self) {
        let now = Instant::now();
        let mut stats = self.stats.lock().unwrap();
        stats.frames += 1;
        stats.first_frame_at.get_or_insert(now);
        stats.last_frame_at = Some(now);
        if stats.released_at.is_some() {
            stats.first_after_release_at.get_or_insert(now);
            stats.frames_after_release += 1;
        }
        let epoch = *self.epoch.lock().unwrap();
        let second = match now.checked_duration_since(epoch) {
            Some(after) => i64::try_from(after.as_secs()).unwrap_or(i64::MAX),
            None => -i64::try_from((epoch - now).as_secs()).unwrap_or(i64::MAX) - 1,
        };
        *stats.per_second.entry(second).or_default() += 1;
    }
}

/// The dequeue the room loop makes, behind the gate. Generic so the gate is
/// testable on a stream that a test controls.
pub(super) async fn gated_next<S: Stream + Unpin>(
    gate: Option<&DequeueGate>,
    stream: &mut S,
) -> Option<S::Item> {
    if let Some(gate) = gate {
        gate.wait_open().await;
    }
    let item = stream.next().await;
    if item.is_some()
        && let Some(gate) = gate
    {
        gate.record_frame();
    }
    item
}

/// One input transcript fragment as the room loop received it.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct TranscriptLine {
    pub(super) at: Instant,
    pub(super) text: String,
}

/// The loop's own audio counters, summed over the steps the probe saw.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Retention {
    /// The largest cumulative ingress drop count seen, in bytes.
    pub(super) dropped_bytes: usize,
    /// Bytes quarantined, summed from each step's fresh increment.
    pub(super) quarantined_bytes: usize,
}

pub(super) struct ContinuityProbe {
    pub(super) gate: DequeueGate,
    pub(super) epoch: Instant,
    transcripts: Mutex<Vec<TranscriptLine>>,
    retention: Mutex<Retention>,
}

impl ContinuityProbe {
    pub(super) fn new() -> Arc<Self> {
        let epoch = Instant::now();
        Arc::new(Self {
            gate: DequeueGate::new(epoch),
            epoch,
            transcripts: Mutex::new(Vec::new()),
            retention: Mutex::new(Retention::default()),
        })
    }

    pub(super) fn transcripts(&self) -> Vec<TranscriptLine> {
        self.transcripts.lock().unwrap().clone()
    }

    pub(super) fn retention(&self) -> Retention {
        self.retention.lock().unwrap().clone()
    }
}

static ACTIVE: Mutex<Option<Arc<ContinuityProbe>>> = Mutex::new(None);

/// Keeps a probe installed for the room loop, and removes it on drop.
pub(super) struct Installed(Arc<ContinuityProbe>);

impl Drop for Installed {
    fn drop(&mut self) {
        let mut active = ACTIVE.lock().unwrap();
        if active.as_ref().is_some_and(|now| Arc::ptr_eq(now, &self.0)) {
            *active = None;
        }
    }
}

/// Installs `probe` for every room loop in this process. A fixture that does
/// this runs alone in its process: the hooks are process-wide.
pub(super) fn install(probe: &Arc<ContinuityProbe>) -> Installed {
    *ACTIVE.lock().unwrap() = Some(probe.clone());
    Installed(probe.clone())
}

fn active() -> Option<Arc<ContinuityProbe>> {
    ACTIVE.lock().unwrap().clone()
}

/// The dequeue the room loop's audio consumer makes.
pub(super) async fn next_through_probe<S: Stream + Unpin>(stream: &mut S) -> Option<S::Item> {
    let probe = active();
    gated_next(probe.as_deref().map(|probe| &probe.gate), stream).await
}

pub(super) fn note_transcript(text: &str) {
    if let Some(probe) = active() {
        probe.transcripts.lock().unwrap().push(TranscriptLine {
            at: Instant::now(),
            text: text.to_owned(),
        });
    }
}

pub(super) fn note_retention(dropped_bytes: usize, quarantined_bytes: usize) {
    if let Some(probe) = active() {
        let mut retention = probe.retention.lock().unwrap();
        retention.dropped_bytes = retention.dropped_bytes.max(dropped_bytes);
        retention.quarantined_bytes = retention
            .quarantined_bytes
            .saturating_add(quarantined_bytes);
    }
}
