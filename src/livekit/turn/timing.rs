//! Observed reply timings, separate from the deadlines that drive recovery.
//!
//! All durations use the room's monotonic clock. A PCM level crossing and a
//! transcription receipt are proxies for speech ending, not acoustic truth.
//! Provider audio offsets are logged separately, never subtracted from that
//! clock. A missing activity-end signal is unknown, not zero VAD latency.

use std::time::{Duration, Instant};

/// How long the microphone has to stay quiet before a checkpoint may go out:
/// above the default one-second end-of-speech silence, so a pause between two
/// sentences is not taken for the end of the turn.
pub(in crate::livekit) const CHECKPOINT_VOICE_QUIET: Duration = Duration::from_secs(2);

#[derive(Default)]
pub(in crate::livekit) struct TurnTiming {
    sequence: u64,
    pending: Option<Pending>,
    audio_reported: bool,
    last_voice: Option<Instant>,
    completed: bool,
    incoming: Option<IncomingActivity>,
}

struct IncomingActivity {
    voice: Option<Instant>,
    closed: Option<(Instant, Option<Duration>)>,
}

struct Pending {
    id: u64,
    candidate: bool,
    model_started: bool,
    start: Option<Instant>,
    start_source: &'static str,
    last_transcript: Option<Instant>,
    closed: Option<Instant>,
    audio_offset: Option<Duration>,
    first_tool: Option<Instant>,
    tool_calls: u64,
    tool_exec: Duration,
    tool_response: Duration,
    tool_response_errors: u64,
    tool_spans: Vec<(Instant, Instant)>,
}

impl Pending {
    fn frozen(&self) -> bool {
        self.closed.is_some() || self.model_started
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate::livekit) enum TimingOutcome {
    Audio,
    NoAudio,
    Interrupted,
    Replaced,
    Held,
    Ended,
}

impl TimingOutcome {
    fn label(self) -> &'static str {
        match self {
            Self::Audio => "audio",
            Self::NoAudio => "no_audio",
            Self::Interrupted => "interrupted",
            Self::Replaced => "replaced",
            Self::Held => "held",
            Self::Ended => "ended",
        }
    }
}

pub(in crate::livekit) struct TimingSummary {
    pub(in crate::livekit) id: u64,
    pub(in crate::livekit) wait: Option<Duration>,
    pub(in crate::livekit) elapsed: Option<Duration>,
    pub(in crate::livekit) start_source: &'static str,
    pub(in crate::livekit) turn_close: Option<Duration>,
    pub(in crate::livekit) turn_close_reason: &'static str,
    pub(in crate::livekit) transcript_wait: Option<Duration>,
    pub(in crate::livekit) audio_offset: Option<Duration>,
    pub(in crate::livekit) tool_calls: u64,
    pub(in crate::livekit) pre_tool: Option<Duration>,
    pub(in crate::livekit) tool_phase: Option<Duration>,
    pub(in crate::livekit) tool_exec: Duration,
    pub(in crate::livekit) tool_response: Duration,
    pub(in crate::livekit) tool_response_errors: u64,
    pub(in crate::livekit) tool_wait: Option<Duration>,
    pub(in crate::livekit) tool_after_close: Option<Duration>,
    pub(in crate::livekit) other: Option<Duration>,
    outcome: TimingOutcome,
}

impl TimingSummary {
    pub(in crate::livekit) fn log_line(
        &self,
        room: &str,
        session: u64,
        socket: u64,
        at: &str,
    ) -> String {
        format!(
            "codetrial turn_timing room={room} session={session} socket={socket} turn={} at={at} \
             start_source={} wait_ms={} elapsed_ms={} transcript_wait_ms={} turn_close_ms={} turn_close_source={} turn_close_reason={} \
             vad_audio_offset_ms={} tool_calls={} pre_tool_ms={} tool_phase_ms={} tool_exec_ms={} tool_response_ms={} \
             tool_response_errors={} tool_wait_ms={} tool_after_close_ms={} other_ms={} outcome={}",
            self.id,
            self.start_source,
            milliseconds(self.wait),
            milliseconds(self.elapsed),
            milliseconds(self.transcript_wait),
            milliseconds(self.turn_close),
            if self.turn_close.is_some() {
                "voice_activity_received"
            } else {
                "unknown"
            },
            self.turn_close_reason,
            milliseconds(self.audio_offset),
            self.tool_calls,
            milliseconds(self.pre_tool),
            milliseconds(self.tool_phase),
            self.tool_exec.as_millis(),
            self.tool_response.as_millis(),
            self.tool_response_errors,
            milliseconds(self.tool_wait),
            milliseconds(self.tool_after_close),
            milliseconds(self.other),
            self.outcome.label(),
        )
    }
}

fn milliseconds(duration: Option<Duration>) -> String {
    duration.map_or_else(
        || "unknown".to_string(),
        |value| value.as_millis().to_string(),
    )
}

/// Clips work to one wait window and counts overlapping intervals only once.
/// A reversed interval makes attribution unknown rather than hiding bad data.
fn span_time(spans: &[(Instant, Instant)], from: Instant, to: Instant) -> Option<Duration> {
    if to.checked_duration_since(from).is_none()
        || spans
            .iter()
            .any(|(start, end)| end.checked_duration_since(*start).is_none())
    {
        return None;
    }
    let mut clipped = spans
        .iter()
        .filter_map(|(start, end)| {
            let start = (*start).max(from);
            let end = (*end).min(to);
            end.checked_duration_since(start).map(|_| (start, end))
        })
        .collect::<Vec<_>>();
    clipped.sort_unstable();
    let mut covered_until = from;
    let mut total = Duration::ZERO;
    for (start, end) in clipped {
        let start = start.max(covered_until);
        if let Some(duration) = end.checked_duration_since(start) {
            total += duration;
            covered_until = end;
        }
    }
    Some(total)
}

impl TurnTiming {
    pub(in crate::livekit) fn reset(&mut self) {
        self.pending = None;
        self.audio_reported = false;
        self.last_voice = None;
        self.completed = false;
        self.incoming = None;
    }

    fn pending(&mut self) -> &mut Pending {
        self.pending.get_or_insert_with(|| {
            self.sequence += 1;
            Pending {
                id: self.sequence,
                candidate: false,
                model_started: false,
                start: None,
                start_source: "unknown",
                last_transcript: None,
                closed: None,
                audio_offset: None,
                first_tool: None,
                tool_calls: 0,
                tool_exec: Duration::ZERO,
                tool_response: Duration::ZERO,
                tool_response_errors: 0,
                tool_spans: Vec::new(),
            }
        })
    }

    pub(in crate::livekit) fn note_voice(&mut self, now: Instant) {
        self.last_voice = Some(now);
        if let Some(incoming) = &mut self.incoming
            && incoming.closed.is_none()
        {
            incoming.voice = Some(now);
        }
        if let Some(pending) = &mut self.pending
            && pending.candidate
            && !pending.frozen()
        {
            pending.start = Some(now);
            pending.start_source = "pcm_level";
        }
    }

    /// A provider-confirmed continuation can reopen speech before generation.
    /// During generation, keep the observation for the next measurement while
    /// the room processes its original activity-before-interruption order.
    pub(in crate::livekit) fn note_activity_start(&mut self, now: Instant) {
        if self.audio_reported
            || self
                .pending
                .as_ref()
                .is_some_and(|pending| pending.model_started)
        {
            let previous = self
                .pending
                .as_ref()
                .and_then(|pending| pending.closed.or(pending.start));
            self.incoming = Some(IncomingActivity {
                voice: self
                    .recent_voice(now)
                    .filter(|voice| previous.is_none_or(|at| *voice > at)),
                closed: None,
            });
            return;
        }
        self.audio_reported = false;
        self.completed = false;

        // The start signal can lag a short utterance's PCM. Keep levels that
        // arrived after the previous close, but never borrow its last word.
        if self
            .pending
            .as_ref()
            .and_then(|pending| pending.closed)
            .is_some_and(|closed| self.last_voice.is_some_and(|voice| voice <= closed))
        {
            self.last_voice = None;
        }
        let last_voice = self.recent_voice(now);
        let pending = self.pending();
        pending.candidate = true;
        pending.start = last_voice;
        pending.start_source = if last_voice.is_some() {
            "pcm_level"
        } else {
            "unknown"
        };
        pending.last_transcript = None;
        pending.closed = None;
        pending.audio_offset = None;
    }

    /// Called only when the existing turn guard arms a candidate reply wait.
    /// Late transcripts of an answer already being generated do not get here.
    pub(in crate::livekit) fn note_transcript(&mut self, now: Instant) {
        self.audio_reported = false;
        self.completed = false;
        let last_voice = self.recent_voice(now);
        let pending = self.pending();
        pending.candidate = true;
        pending.last_transcript = Some(now);
        if !pending.frozen() {
            pending.start = Some(last_voice.unwrap_or(now));
            pending.start_source = if last_voice.is_some() {
                "pcm_level"
            } else {
                "last_transcript"
            };
        } else if pending.start.is_none() {
            // PCM received after a frozen boundary cannot stand in for the
            // original speech. A late transcript is an explicit fallback.
            pending.start = Some(now);
            pending.start_source = "last_transcript";
        }
    }

    pub(in crate::livekit) fn note_activity_end(
        &mut self,
        now: Instant,
        audio_offset: Option<Duration>,
    ) {
        if let Some(incoming) = &mut self.incoming {
            incoming.closed.get_or_insert((now, audio_offset));
            return;
        }
        if self.audio_reported {
            return;
        }
        self.completed = false;
        let last_voice = self.recent_voice(now);
        let pending = self.pending();
        pending.candidate = true;
        if !pending.frozen()
            && let Some(voice) = last_voice
        {
            pending.start = Some(voice);
            pending.start_source = "pcm_level";
        }
        if pending.closed.is_none() {
            pending.closed = Some(now);
            pending.audio_offset = audio_offset;
        }
    }

    pub(in crate::livekit) fn note_model_output(&mut self) {
        if !self.audio_reported {
            self.completed = false;
            self.pending().model_started = true;
        }
    }

    pub(in crate::livekit) fn note_output_transcript(&mut self) {
        // Provider transcription can trail TurnComplete. It can describe an
        // open measurement, but cannot establish a new one after that boundary.
        if !self.completed || self.pending.is_some() {
            self.note_model_output();
        }
    }

    fn recent_voice(&self, now: Instant) -> Option<Instant> {
        self.last_voice.filter(|at| {
            now.checked_duration_since(*at)
                .is_some_and(|age| age < CHECKPOINT_VOICE_QUIET)
        })
    }

    pub(in crate::livekit) fn finish_thinking_change(
        &mut self,
        now: Instant,
        changed: Option<bool>,
    ) -> Option<TimingSummary> {
        match changed {
            Some(true) => self.finish(now, TimingOutcome::Held),
            _ => None,
        }
    }

    pub(in crate::livekit) fn note_tool_execution(&mut self, started: Instant, finished: Instant) {
        if self.audio_reported {
            return;
        }
        let pending = self.pending();
        pending.model_started = true;
        pending.tool_calls += 1;
        pending.tool_exec += finished.checked_duration_since(started).unwrap_or_default();
        pending.tool_spans.push((started, finished));
    }

    pub(in crate::livekit) fn note_tool_response(
        &mut self,
        started: Instant,
        finished: Instant,
        sent: bool,
    ) {
        if let Some(pending) = &mut self.pending {
            pending.tool_response += finished.checked_duration_since(started).unwrap_or_default();
            pending.tool_spans.push((started, finished));
            pending.tool_response_errors += u64::from(!sent);
        }
    }

    pub(in crate::livekit) fn note_tool_request(&mut self, now: Instant) {
        if !self.audio_reported {
            let pending = self.pending();
            pending.model_started = true;
            pending.first_tool.get_or_insert(now);
        }
    }

    pub(in crate::livekit) fn note_audio(
        &mut self,
        now: Instant,
        accepted: bool,
    ) -> Option<TimingSummary> {
        if accepted {
            self.finish(now, TimingOutcome::Audio)
        } else {
            None
        }
    }

    pub(in crate::livekit) fn finish(
        &mut self,
        now: Instant,
        outcome: TimingOutcome,
    ) -> Option<TimingSummary> {
        if outcome == TimingOutcome::Audio {
            if self.audio_reported {
                return None;
            }

            // Greetings and replies to platform prompts have no candidate
            // speech timestamp, but still deserve tool counts and a turn ID.
            self.pending();
        }
        let pending = self.pending.take();
        self.audio_reported = outcome == TimingOutcome::Audio;
        self.last_voice = None;
        if outcome != TimingOutcome::Audio {
            self.completed = true;
            let incoming = self.incoming.take();
            if matches!(outcome, TimingOutcome::Interrupted | TimingOutcome::NoAudio)
                && let Some(incoming) = incoming
            {
                self.completed = false;
                self.last_voice = incoming.voice;
                let next = self.pending();
                next.candidate = true;
                next.start = incoming.voice;
                next.start_source = if incoming.voice.is_some() {
                    "pcm_level"
                } else {
                    "unknown"
                };
                if let Some((closed, offset)) = incoming.closed {
                    next.closed = Some(closed);
                    next.audio_offset = offset;
                }
            }
        }
        let pending = pending?;
        let elapsed = pending
            .start
            .and_then(|start| now.checked_duration_since(start));
        let (turn_close, turn_close_reason) = match (pending.start, pending.closed) {
            (None, _) => (None, "no_start"),
            (_, None) => (None, "not_observed"),
            (Some(start), Some(closed)) if closed < start => (None, "before_start"),
            (_, Some(closed)) if closed > now => (None, "after_audio"),
            (Some(start), Some(closed)) => (Some(closed.duration_since(start)), "observed"),
        };
        let audio = outcome == TimingOutcome::Audio;
        let tool_wait = if audio {
            pending
                .start
                .and_then(|start| span_time(&pending.tool_spans, start, now))
        } else {
            None
        };
        let tool_after_close = if audio && turn_close.is_some() {
            pending
                .closed
                .and_then(|closed| span_time(&pending.tool_spans, closed, now))
        } else {
            None
        };
        let other = elapsed.and_then(|wait| {
            wait.checked_sub(turn_close?)?
                .checked_sub(tool_after_close?)
        });
        Some(TimingSummary {
            id: pending.id,
            wait: if outcome == TimingOutcome::Audio {
                elapsed
            } else {
                None
            },
            elapsed,
            start_source: pending.start_source,
            turn_close,
            turn_close_reason,
            transcript_wait: if audio {
                pending
                    .last_transcript
                    .and_then(|at| now.checked_duration_since(at))
            } else {
                None
            },
            audio_offset: pending.audio_offset,
            tool_calls: pending.tool_calls,
            pre_tool: pending
                .start
                .and_then(|start| pending.first_tool?.checked_duration_since(start)),
            tool_phase: if outcome == TimingOutcome::Audio {
                pending
                    .first_tool
                    .and_then(|first| now.checked_duration_since(first))
            } else {
                None
            },
            tool_exec: pending.tool_exec,
            tool_response: pending.tool_response,
            tool_response_errors: pending.tool_response_errors,
            tool_wait,
            tool_after_close,
            other,
            outcome,
        })
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/livekit/turn/timing.rs"]
mod tests;
