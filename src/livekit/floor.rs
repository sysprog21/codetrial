//! Provider activity observations only; never used to drive the interview.
use std::time::{Duration, Instant};

pub(super) const CUT_OFF_MS: u128 = 1500;
pub(super) const FALSE_STOP_MS: u128 = 600;
pub(super) const MISSED_BARGE_IN_MS: u128 = 1500;

pub(super) enum Event {
    Start { generating: bool, ahead: Duration },
    End,
    Output { audio: bool },
    Complete { playing: bool },
    Drained,
    Interrupted,
    ActivityLost,
    Reset,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct Record {
    pub seq: u64,
    pub opened_at: Instant,
    pub resolved_at: Option<Instant>,
    pub activity_end_at: Option<Instant>,
    pub kind: &'static str,
    pub outcome: &'static str,
    pub gap_ms: Option<u128>,
    pub overlap_ms: Option<u128>,
    pub playout_ahead_ms: Option<u128>,
    pub suspect: Option<&'static str>,
}

impl Record {
    pub(super) fn log_line(&self, room: &str, session: u64, socket: u64, clock: &str) -> String {
        fn value(ms: Option<u128>) -> String {
            ms.map_or_else(|| "unknown".into(), |ms| ms.to_string())
        }
        format!(
            "codetrial floor room={room} session={session} socket={socket} seq={} kind={} outcome={} at={clock} gap_ms={} overlap_ms={} playout_ahead_ms={} suspect={}",
            self.seq,
            self.kind,
            self.outcome,
            value(self.gap_ms),
            value(self.overlap_ms),
            value(self.playout_ahead_ms),
            self.suspect.unwrap_or("unknown")
        )
    }
}

struct Pause {
    record: Record,
    start: Instant,
    audio: Option<Instant>,
    audio_generation_open: bool,
}

fn ms_since(now: Instant, from: Instant) -> Option<u128> {
    now.checked_duration_since(from).map(|d| d.as_millis())
}

/// Settled once its outcome leaves `unresolved`.
struct Overlap {
    record: Record,
    start: Instant,
    ended: Option<Instant>,
    /// When playout drained without an interruption. The agent stopped talking
    /// here, so the candidate only spoke over it up to this point.
    drained: Option<Instant>,
}

#[derive(Default)]
pub(super) struct FloorTracker {
    sequence: u64,
    active: bool,
    output_started: bool,
    pause: Option<Pause>,
    overlap: Option<Overlap>,
}

impl FloorTracker {
    fn record(&mut self, kind: &'static str, now: Instant) -> Record {
        self.sequence += 1;
        Record {
            seq: self.sequence,
            opened_at: now,
            resolved_at: None,
            activity_end_at: None,
            kind,
            outcome: "unresolved",
            gap_ms: None,
            overlap_ms: None,
            playout_ahead_ms: None,
            suspect: None,
        }
    }

    fn finish_overlap(&mut self, emit: &mut impl FnMut(Record)) {
        if let Some(mut overlap) = self
            .overlap
            .take_if(|o| o.ended.is_some() && o.record.outcome != "unresolved")
        {
            // A continuation counts only the speech that ran while the agent
            // still talked, not speech that went on after playout drained.
            let talked_over = overlap.ended.and_then(|end| {
                let until = overlap.drained.map_or(end, |drained| drained.min(end));
                ms_since(until, overlap.start)
            });
            overlap.record.suspect = match (overlap.record.outcome, overlap.record.overlap_ms) {
                ("interrupted", Some(ms)) if ms < FALSE_STOP_MS => Some("false_stop"),
                ("continued", _) if talked_over.is_some_and(|ms| ms >= MISSED_BARGE_IN_MS) => {
                    Some("missed_barge_in")
                }
                _ => None,
            };
            emit(overlap.record);
        }
    }

    /// At most one pause and one overlap are retained, even without provider
    /// END.
    /// A responded pause waits for START to classify cut_off using first audio.
    pub(super) fn observe(&mut self, event: Event, now: Instant, emit: &mut impl FnMut(Record)) {
        match event {
            Event::Start { generating, ahead } => {
                if self.active {
                    return;
                }
                self.active = true;
                if let Some(mut pause) = self.pause.take() {
                    if pause.record.outcome == "unresolved" {
                        pause.record.outcome = "continue";
                        pause.record.resolved_at = Some(now);
                        pause.record.gap_ms = ms_since(now, pause.start);
                    } else if pause
                        .audio
                        .and_then(|at| ms_since(now, at))
                        .is_some_and(|gap| gap <= CUT_OFF_MS)
                    {
                        pause.record.suspect = Some("cut_off");
                    }
                    emit(pause.record);
                }

                // A missing END must not stretch an old overlap into a new
                // utterance.
                if let Some(overlap) = self.overlap.take() {
                    emit(overlap.record);
                }
                if generating || !ahead.is_zero() {
                    let mut record = self.record("overlap", now);
                    record.playout_ahead_ms = Some(ahead.as_millis());
                    self.overlap = Some(Overlap {
                        record,
                        start: now,
                        ended: None,
                        drained: None,
                    });
                }
            }
            Event::End => {
                if let Some(overlap) = &mut self.overlap
                    && overlap.ended.is_none()
                {
                    overlap.ended = Some(now);
                    overlap.record.activity_end_at = Some(now);
                    overlap.record.overlap_ms = ms_since(now, overlap.start);
                }
                self.finish_overlap(emit);
                if !self.active
                    && self
                        .pause
                        .as_ref()
                        .is_some_and(|p| p.record.outcome == "unresolved")
                {
                    return;
                }
                self.active = false;
                if let Some(pause) = self.pause.take() {
                    emit(pause.record);
                }
                let record = self.record("pause", now);
                self.pause = Some(Pause {
                    record,
                    start: now,
                    audio: None,
                    audio_generation_open: true,
                });
            }
            Event::Output { audio } => {
                let first_output = !self.output_started;
                self.output_started = true;
                if let Some(pause) = &mut self.pause {
                    if first_output && pause.record.outcome == "unresolved" {
                        pause.record.outcome = "respond";
                        pause.record.resolved_at = Some(now);
                        pause.record.gap_ms = ms_since(now, pause.start);
                    }
                    // `get_or_insert` keeps the generation's first chunk.
                    if audio && pause.record.outcome == "respond" && pause.audio_generation_open {
                        pause.audio.get_or_insert(now);
                    }
                }
            }
            Event::Interrupted => {
                if let Some(overlap) = &mut self.overlap
                    && overlap.record.outcome == "unresolved"
                {
                    overlap.record.outcome = "interrupted";
                    overlap.record.resolved_at = Some(now);
                }
                self.finish_overlap(emit);
            }
            Event::Complete { playing } => {
                self.output_started = false;
                if let Some(pause) = self.pause.take_if(|p| p.record.outcome == "unresolved") {
                    emit(pause.record);
                }
                if let Some(pause) = &mut self.pause {
                    // A tool-only response must not borrow a later generation's
                    // audio.
                    pause.audio_generation_open = false;
                }
                if !playing {
                    self.observe(Event::Drained, now, emit);
                }
            }
            Event::Drained => {
                if let Some(overlap) = &mut self.overlap
                    && overlap.record.outcome == "unresolved"
                {
                    overlap.record.outcome = "continued";
                    overlap.record.resolved_at = Some(now);
                    overlap.drained = Some(now);
                }
                self.finish_overlap(emit);
            }
            Event::ActivityLost => {
                if self.active {
                    self.active = false;
                    if let Some(mut overlap) = self.overlap.take() {
                        overlap.record.outcome = "unresolved";
                        overlap.record.resolved_at = None;
                        overlap.record.overlap_ms = None;
                        emit(overlap.record);
                    }
                }
            }
            Event::Reset => {
                self.active = false;
                self.output_started = false;
                if let Some(pause) = self.pause.take() {
                    emit(pause.record);
                }
                if let Some(mut overlap) = self.overlap.take() {
                    overlap.record.outcome = "unresolved";
                    overlap.record.resolved_at = None;
                    emit(overlap.record);
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "../../tests/unit/livekit/floor.rs"]
mod tests;
