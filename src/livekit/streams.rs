//! What the candidate's tracks deliver: the receivers, the queue that holds
//! their audio across a replacement, and the voice stamp the camera gate
//! reads. A leaf under the media pump and the turn state, which both name it.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use ::livekit::webrtc::audio_frame::AudioFrame;
use ::livekit::webrtc::audio_stream::native::NativeAudioStream;
use ::livekit::webrtc::video_stream::native::NativeVideoStream;

pub(in crate::livekit) const GEMINI_AUDIO_SAMPLE_RATE: i32 = 16_000;

/// Forty seconds of ten-millisecond frames: the SDK queue and the recovery
/// buffer hold the same span so neither drops what the other keeps.
pub(in crate::livekit) const CANDIDATE_AUDIO_QUEUE_FRAMES: usize = 4000;

pub(in crate::livekit) const INGRESS_FRAMES: usize = CANDIDATE_AUDIO_QUEUE_FRAMES;
pub(in crate::livekit) const INGRESS_AGE: Duration = Duration::from_secs(40);
/// The same window in bytes of 16-bit mono input, so the age and size bounds
/// cannot drift apart.
pub(in crate::livekit) const INGRESS_BYTES: usize =
    INGRESS_AGE.as_secs() as usize * GEMINI_AUDIO_SAMPLE_RATE as usize * 2;

/// Bytes of 16-bit mono input in `frame`: the unit `INGRESS_BYTES` is in.
fn frame_bytes(frame: &AudioFrame<'_>) -> usize {
    frame.data.len().saturating_mul(2)
}

#[derive(Default)]
pub(in crate::livekit) struct Ingress {
    pub(in crate::livekit) frames: VecDeque<(Instant, CandidateInput)>,
    pub(in crate::livekit) bytes: usize,
    leading_yield: bool,
    pub(super) dropped_bytes: usize,
    logged_dropped_bytes: usize,
}

impl Ingress {
    /// The drops since the last time this was asked for. `dropped_bytes` is
    /// cumulative across replacements, and the analyzer sums the log line per
    /// room, so each line carries only what the previous one did not.
    pub(super) fn take_unlogged_drops(&mut self) -> usize {
        let new = self.dropped_bytes.saturating_sub(self.logged_dropped_bytes);
        self.logged_dropped_bytes = self.dropped_bytes;
        new
    }

    pub(super) fn is_empty(&self) -> bool {
        self.frames.is_empty() && !self.leading_yield
    }

    pub(super) fn has_audio(&mut self) -> bool {
        self.expire(Instant::now());
        self.bytes != 0
    }

    fn discard_front(&mut self) {
        if let Some((_, frame)) = self.frames.pop_front() {
            let bytes = match frame {
                CandidateInput::BufferedFrame(frame) => frame_bytes(&frame),
                CandidateInput::Yield => {
                    self.leading_yield = true;
                    0
                }
                _ => 0,
            };
            self.bytes -= bytes;
            self.dropped_bytes = self.dropped_bytes.saturating_add(bytes);
        }
    }

    /// Trimming drops from the front at most once per frame held, so a
    /// condition that could never become false ends with the queue empty
    /// instead of spinning.
    fn expire(&mut self, now: Instant) {
        for _ in 0..self.frames.len() {
            let expired = self
                .frames
                .front()
                .is_some_and(|(at, _)| now.saturating_duration_since(*at) >= INGRESS_AGE);
            if !expired {
                break;
            }
            self.discard_front();
        }
    }

    pub(super) fn push(&mut self, frame: AudioFrame<'static>, now: Instant) {
        self.expire(now);
        let bytes = frame_bytes(&frame);
        if bytes == 0 {
            return;
        }
        if bytes > INGRESS_BYTES {
            self.dropped_bytes = self.dropped_bytes.saturating_add(bytes);
            return;
        }
        for _ in 0..self.frames.len() {
            if self.frames.len() < INGRESS_FRAMES && self.bytes + bytes <= INGRESS_BYTES {
                break;
            }
            self.discard_front();
        }
        self.bytes += bytes;
        self.frames
            .push_back((now, CandidateInput::BufferedFrame(frame)));
    }

    pub(super) fn pop(&mut self, now: Instant) -> Option<CandidateInput> {
        self.expire(now);
        if std::mem::take(&mut self.leading_yield) {
            return Some(CandidateInput::Yield);
        }
        let (_, input) = self.frames.pop_front()?;
        if let CandidateInput::BufferedFrame(frame) = &input {
            self.bytes -= frame_bytes(frame);
        }
        Some(input)
    }

    pub(super) fn yielded(&mut self, now: Instant) {
        self.expire(now);
        if (self.leading_yield && self.frames.is_empty())
            || self
                .frames
                .back()
                .is_some_and(|(_, item)| matches!(item, CandidateInput::Yield))
        {
            return;
        }
        for _ in 0..self.frames.len() {
            if self.frames.len() < INGRESS_FRAMES {
                break;
            }
            self.discard_front();
        }
        self.frames.push_back((now, CandidateInput::Yield));
    }
}

pub(in crate::livekit) enum CandidateInput {
    Frame(AudioFrame<'static>),
    BufferedFrame(AudioFrame<'static>),
    Yield,
    End,
}

/// What the candidate's tracks deliver, kept beside [`CandidateMedia`] and not
/// in it: the room loop's `select!` arms hold their futures at the same time,
/// and each needs a borrow of its own receiver while the arms' handlers use
/// the media around them. A bundle of receivers, not a type with rules.
#[derive(Default)]
pub(in crate::livekit) struct CandidateStreams {
    pub(super) recovery_audio: Ingress,
    pub(super) audio: Option<NativeAudioStream>,
    pub(super) video: Option<NativeVideoStream>,
    /// When the microphone last carried voice, measured on every frame. The
    /// camera gate reads this rather than provider speech events, which a
    /// session may never receive.
    pub(super) voiced_at: Option<Instant>,
}
