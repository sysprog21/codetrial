//! Audio and video, in both directions.
//!
//! Split from the room loop because none of it is about the interview: these
//! are the buffers, codecs and pumps that carry the candidate's microphone and
//! camera to Gemini and Gemini's voice back to the room. The room decides when
//! a turn starts; this decides what a frame becomes on the way.

use std::time::{Duration, Instant};

use ::livekit::options::TrackPublishOptions;
use ::livekit::prelude::{
    LocalAudioTrack, LocalTrack, RemoteAudioTrack, RemoteParticipant, RemoteTrack, Room, RoomEvent,
    RtcAudioSource, TrackSource,
};
use ::livekit::webrtc::audio_frame::AudioFrame;
use ::livekit::webrtc::audio_source::AudioSourceOptions;
use ::livekit::webrtc::audio_source::native::NativeAudioSource;
use ::livekit::webrtc::audio_stream::native::{NativeAudioStream, NativeAudioStreamOptions};
use ::livekit::webrtc::audio_track::RtcAudioTrack;
use ::livekit::webrtc::video_frame::{BoxVideoFrame, VideoFormatType};
use ::livekit::webrtc::video_stream::native::NativeVideoStream;
use futures_util::StreamExt;
use jpeg_encoder::{ColorType, Encoder};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::is_interview_participant;
use crate::gemini::GeminiLiveSession;

pub(super) const GEMINI_AUDIO_SAMPLE_RATE: i32 = 16_000;

pub(super) const GEMINI_AUDIO_CHANNELS: i32 = 1;

/// Forty seconds of ten-millisecond frames: the SDK queue and the recovery
/// buffer hold the same span so neither drops what the other keeps.
pub(super) const CANDIDATE_AUDIO_QUEUE_FRAMES: usize = 4000;

pub(super) fn candidate_audio_stream(track: RtcAudioTrack) -> NativeAudioStream {
    NativeAudioStream::with_options(
        track,
        GEMINI_AUDIO_SAMPLE_RATE,
        GEMINI_AUDIO_CHANNELS,
        NativeAudioStreamOptions {
            // Forty seconds covers partial transcription and a bounded
            // connection/setup attempt without the default 100 ms overflow.
            queue_size_frames: Some(CANDIDATE_AUDIO_QUEUE_FRAMES),
        },
    )
}

/// Forward each complete ten-millisecond LiveKit frame as 16 kHz mono PCM16.
/// Combining two frames held the first for another ten milliseconds, including
/// an utterance ending on an odd frame. This doubles websocket message count
/// from 50 to 100 per second; the audio is unchanged, and only the framing
/// each message carries is added. A shorter frame still waits in the buffer
/// for the rest of one.
pub(super) const GEMINI_AUDIO_BUFFER_BYTES: usize = 320;

/// One frame in five seconds. Each frame stays in the Live context and is
/// billed again on every later turn, and a presence check needs no more.
pub(super) const GEMINI_VIDEO_FRAME_INTERVAL: Duration = Duration::from_secs(5);

pub(super) const GEMINI_VIDEO_JPEG_QUALITY: u8 = 75;

pub(super) const LIVEKIT_OUTPUT_CHANNELS: u32 = 1;

pub(super) const LIVEKIT_OUTPUT_QUEUE_MS: u32 = 100;

pub(super) const LIVEKIT_OUTPUT_FRAME_QUEUE: usize = 6_000;

/// Buffers one arriving audio frame, and flushes when there is enough to send.
///
/// `None` is the track ending: what is buffered goes now, because nothing else
/// is going to arrive to push it over the threshold.
pub(super) async fn pump_audio<S: AudioSink>(
    media: &mut CandidateMedia,
    gemini: &mut S,
    frame: Option<AudioFrame<'static>>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let Some(frame) = frame else {
        // Only the flush. Letting go of the stream is the caller's, because it
        // has to happen whether or not this fails: the caller logs a write
        // error and keeps looping, and a stream still in place is polled again
        // at once, ends again and fails again. Both `select!` arms are guarded
        // on the stream still being present, so that is a busy loop for as long
        // as the room lasts, not a lost frame.
        flush_candidate_audio(gemini, media).await?;
        return Ok(());
    };

    // Not `last_user_speech`. LiveKit delivers frames for as long as the track
    // is live, silence included, so stamping here made the interview
    // permanently "just spoke": `idle_seconds` never reached
    // SILENCE_THRESHOLD_S and the nudge never fired. Real speech is stamped on
    // `InputTranscript`, where Gemini has already decided that words were said.
    append_pcm16_bytes(&frame, &mut media.audio_bytes);
    if media.audio_bytes.len() >= GEMINI_AUDIO_BUFFER_BYTES {
        flush_candidate_audio(gemini, media).await?;
    }
    Ok(())
}

/// Generic so it can be tested: a real stream needs a real track, so a rule
/// written directly against `NativeAudioStream` is one no test can reach, and
/// this rule is the whole of the bug.
pub(super) fn release_if_ended<T>(stream: &mut Option<T>, ended: bool) {
    if ended {
        *stream = None;
    }
}

/// Sends one arriving video frame, at most one per interval, and outside the
/// candidate's speech only the socket's first. On 3.1 Live every frame counts
/// toward the turn and stays in the context billed each turn after, while a
/// presence check needs only the frames that accompany what was said. Images
/// of the board take their own path and are not held back by this.
pub(super) async fn pump_video(
    media: &mut CandidateMedia,
    gemini: &mut GeminiLiveSession,
    frame: Option<BoxVideoFrame>,
    candidate_speaking: bool,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let Some(frame) = frame else {
        media.video = None;
        return Ok(());
    };
    if !should_send_video_frame(
        media.last_video_frame.elapsed(),
        candidate_speaking,
        media.presence_frame_sent,
    ) {
        return Ok(());
    }

    // Stamped on the attempt, not the success: the dimensions come off the
    // wire, and a source stuck emitting frames that will not encode must cost
    // one try per interval rather than one per arriving frame.
    media.last_video_frame = Instant::now();
    match encode_video_frame_jpeg_off_thread(&frame, GEMINI_VIDEO_JPEG_QUALITY).await {
        Ok(bytes) => {
            gemini
                .send_video_frame(&bytes, crate::gemini::GEMINI_IMAGE_MIME_TYPE)
                .await?;
            media.presence_frame_sent = true;
        }
        Err(error) => eprintln!("skipping unencodable video frame: {error}"),
    }
    Ok(())
}

/// The candidate's inbound media, plus the identity their tracks arrived on.
/// Bundled because every track event touches two or three of these together.
pub(super) struct CandidateMedia {
    pub(super) recovery_audio: super::recovery::Ingress,
    pub(super) audio: Option<NativeAudioStream>,
    pub(super) video: Option<NativeVideoStream>,
    pub(super) audio_bytes: Vec<u8>,
    pub(super) audio_unconfirmed: bool,
    pub(super) quarantined_audio_bytes: usize,
    pub(super) identity: Option<String>,
    last_video_frame: Instant,
    /// Whether the current socket has a camera frame yet. A replacement clears
    /// it, since a cold socket has seen none and a resumed one may predate it.
    pub(super) presence_frame_sent: bool,
    /// When the microphone last carried voice, measured on every frame. The
    /// camera gate reads this rather than provider speech events, which a
    /// session may never receive.
    pub(super) voiced_at: Option<Instant>,
    /// Set when a new audio stream is attached, so the first frame taken from
    /// it is not measured against the last frame of an earlier stream.
    mic_stream_fresh: bool,
}

impl CandidateMedia {
    pub(super) fn new() -> Self {
        Self {
            recovery_audio: super::recovery::Ingress::default(),
            audio: None,
            video: None,
            audio_bytes: Vec::new(),
            audio_unconfirmed: false,
            quarantined_audio_bytes: 0,
            identity: None,

            // Seeded in the past so the first frame sends immediately. The
            // monotonic clock starts at boot, so on a machine that just came up
            // there may be nothing to subtract from.
            last_video_frame: Instant::now()
                .checked_sub(GEMINI_VIDEO_FRAME_INTERVAL)
                .unwrap_or_else(Instant::now),
            presence_frame_sent: false,
            voiced_at: None,
            mic_stream_fresh: false,
        }
    }

    /// A new audio stream is about to be attached: its first frame is not
    /// measured against the last frame of an earlier one.
    pub(super) fn begin_mic_stream(&mut self) {
        self.mic_stream_fresh = true;
    }

    /// Accounts for one microphone dequeue, `frame` being whether it yielded a
    /// frame (`false` is the stream's end). Called from every place that takes
    /// a frame from the SDK queue and from no place that replays a buffered
    /// one.
    pub(super) fn note_mic_dequeue(
        &mut self,
        latency: &mut super::latency::ReplyLatency,
        frame: bool,
        now: Instant,
    ) {
        if std::mem::take(&mut self.mic_stream_fresh) || !frame {
            latency.mic_stream_broken();
        }
        if frame {
            latency.mic_dequeued(now);
        }
    }

    /// Voice within the quiet window a pause between sentences fits in.
    pub(super) fn voiced_recently(&self, now: Instant) -> bool {
        self.voiced_at.is_some_and(|at| {
            now.saturating_duration_since(at) < super::turn::CHECKPOINT_VOICE_QUIET
        })
    }

    /// Never-attempted samples and whole retained frames still precede a new
    /// publication if republish wins the select just after recovery commits.
    /// Only an ambiguous attempted write is quarantined instead of replayed.
    pub(super) fn begin_audio_publication(&mut self) {
        if self.audio_unconfirmed {
            self.quarantined_audio_bytes = self
                .quarantined_audio_bytes
                .saturating_add(self.audio_bytes.len());
            self.audio_bytes.clear();
            self.audio_unconfirmed = false;
        }
        if !self.recovery_audio.is_empty() {
            self.recovery_audio.yielded(Instant::now());
        }
    }

    pub(super) fn attach_audio(
        &mut self,
        track: &RemoteAudioTrack,
        participant: &RemoteParticipant,
    ) {
        self.begin_audio_publication();
        self.identity = Some(participant.identity().to_string());
        self.begin_mic_stream();
        self.audio = Some(candidate_audio_stream(track.rtc_track()));
    }
}

/// The candidate's tracks arriving and going away. Reports whether the event
/// was media, so the caller's match only has to carry what is left.
pub(super) async fn handle_media_event(
    media: &mut CandidateMedia,
    gemini: &mut GeminiLiveSession,
    candidate_identity: &str,
    forward_video_to_gemini: bool,
    event: &RoomEvent,
) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
    match event {
        RoomEvent::TrackSubscribed {
            track: RemoteTrack::Audio(track),
            participant,
            ..
        } if is_interview_participant(Some(&participant.identity().0), candidate_identity) => {
            media.attach_audio(track, participant);
        }
        RoomEvent::TrackSubscribed {
            track: RemoteTrack::Video(track),
            participant,
            ..
        } if candidate_video_frames_go_to_gemini(
            Some(&participant.identity().0),
            candidate_identity,
            forward_video_to_gemini,
        ) =>
        {
            media.video = Some(NativeVideoStream::new(track.rtc_track()));
        }

        // Guarded like the subscribe arms: an unrelated participant leaving
        // must not tear down the stream the candidate is still speaking into.
        RoomEvent::TrackUnsubscribed {
            track: RemoteTrack::Audio(_),
            participant,
            ..
        } if is_interview_participant(Some(&participant.identity().0), candidate_identity) => {
            flush_candidate_audio(gemini, media).await?;
            media.audio = None;
        }
        RoomEvent::TrackUnsubscribed {
            track: RemoteTrack::Video(_),
            participant,
            ..
        } if is_interview_participant(Some(&participant.identity().0), candidate_identity) => {
            media.video = None;
        }
        _ => return Ok(false),
    }
    Ok(true)
}

fn candidate_video_frames_go_to_gemini(
    sender: Option<&str>,
    candidate_identity: &str,
    opt_in: bool,
) -> bool {
    opt_in && is_interview_participant(sender, candidate_identity)
}

/// Where buffered candidate audio goes.
///
/// Named as a trait with one method so the buffering above can be tested. The
/// real implementation is a websocket to Gemini, so a test that took the
/// concrete type could not call `pump_audio` at all, and the flush threshold,
/// the tail flush on a track ending, and the error on a dead socket all went
/// unchecked because the argument was unconstructible rather than because the
/// behaviour was uninteresting.
pub(super) trait AudioSink {
    fn send_pcm_16khz(
        &mut self,
        bytes: &[u8],
    ) -> impl Future<Output = Result<(), Box<dyn std::error::Error + Send + Sync>>>;
}

impl AudioSink for GeminiLiveSession {
    fn send_pcm_16khz(
        &mut self,
        bytes: &[u8],
    ) -> impl Future<Output = Result<(), Box<dyn std::error::Error + Send + Sync>>> {
        self.send_audio_pcm_16khz(bytes)
    }
}

/// Keep provenance before the first suspension: a failed or cancelled write
/// cannot be replayed as if no bytes could have reached the provider.
pub(super) async fn flush_candidate_audio<S: AudioSink>(
    gemini: &mut S,
    media: &mut CandidateMedia,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    media.audio_unconfirmed |= !media.audio_bytes.is_empty();
    flush_audio(gemini, &mut media.audio_bytes).await?;
    media.audio_unconfirmed = false;
    Ok(())
}

pub(super) async fn flush_audio<S: AudioSink>(
    gemini: &mut S,
    bytes: &mut Vec<u8>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if !bytes.is_empty() {
        gemini.send_pcm_16khz(bytes).await?;
        bytes.clear();
    }
    Ok(())
}

pub(super) enum CandidateInput {
    Frame(AudioFrame<'static>),
    BufferedFrame(AudioFrame<'static>),
    Yield,
    End,
}

pub(super) async fn next_candidate_input(
    audio: &mut Option<NativeAudioStream>,
    pending: &mut super::recovery::Ingress,
) -> CandidateInput {
    if let Some(input) = pending.pop(Instant::now()) {
        return input;
    }
    match next_audio_frame(audio).await {
        Some(frame) => CandidateInput::Frame(frame),
        None => CandidateInput::End,
    }
}

pub(super) async fn next_audio_frame(
    stream: &mut Option<NativeAudioStream>,
) -> Option<AudioFrame<'static>> {
    let stream = stream.as_mut()?;

    // A test hosting the room loop may hold this one dequeue; the loop's other
    // arms keep running, and a cancelled wait has taken no frame.
    #[cfg(test)]
    {
        super::probe::next_through_probe(stream).await
    }
    #[cfg(not(test))]
    {
        stream.next().await
    }
}

pub(super) async fn next_video_frame(
    stream: &mut Option<NativeVideoStream>,
) -> Option<BoxVideoFrame> {
    stream.as_mut()?.next().await
}

pub(super) struct OutputAudio {
    pub(super) latency_marker: Option<(serde_json::Value, Instant)>,
    /// The acknowledgement a tool round may play, once it has been generated.
    pub(super) tool_ack: std::sync::Arc<std::sync::OnceLock<Vec<u8>>>,
    /// The request generating it, cancelled with the session so an interview
    /// that ends or fails to open first does not pay for a clip nobody hears.
    pub(super) tool_ack_task: Option<AbortOnDrop>,
    tool_ack_active: bool,
    pub(super) source: NativeAudioSource,
    pub(super) sample_rate: u32,
    pub(super) pending_bytes: Vec<u8>,
    pub(super) playout_deadline: Instant,
    pub(super) frames: mpsc::Sender<QueuedOutputFrame>,
    pub(super) output_cancellation: CancellationToken,
}

/// Aborts a spawned task when dropped, so the task lives no longer than its
/// owner.
pub(super) struct AbortOnDrop(pub(super) tokio::task::AbortHandle);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

pub(super) struct QueuedOutputFrame {
    pub(super) latency_marker: Option<(serde_json::Value, Instant)>,
    pub(super) output_cancellation: CancellationToken,
    pub(super) samples: Vec<i16>,
}

/// Speech generators pad a clip with silence; played in front of a reply, the
/// padding is only delay. Keeps the voiced stretch of 16-bit mono PCM plus 20
/// ms either side, or nothing when no sample is voiced, and ends it on a whole
/// 10 ms frame, padded with silence: a tail shorter than a frame would wait in
/// `pending_bytes` and be spliced onto the front of the reply.
pub(super) fn trim_silence(pcm: &[u8], sample_rate: u32) -> Vec<u8> {
    const VOICED: u16 = 500;
    let samples = decode_pcm16(pcm);
    let voiced = |sample: &i16| sample.unsigned_abs() >= VOICED;
    let (Some(first), Some(last)) = (
        samples.iter().position(voiced),
        samples.iter().rposition(voiced),
    ) else {
        return Vec::new();
    };
    let pad = sample_rate as usize / 50;
    let start = first.saturating_sub(pad);
    let end = (last + pad + 1).min(samples.len());
    let mut clip = pcm[start * 2..end * 2].to_vec();
    let frame_bytes = sample_rate as usize / 100 * 2;
    clip.resize(clip.len().next_multiple_of(frame_bytes), 0);
    clip
}

impl OutputAudio {
    /// Frames sent to the worker and not yet taken by it.
    pub(super) fn queued_frames(&self) -> usize {
        self.frames.max_capacity() - self.frames.capacity()
    }

    pub(super) fn new(
        source: NativeAudioSource,
        sample_rate: u32,
        frames: mpsc::Sender<QueuedOutputFrame>,
    ) -> Self {
        Self {
            source,
            sample_rate,
            latency_marker: None,
            tool_ack: Default::default(),
            tool_ack_task: None,
            tool_ack_active: false,
            pending_bytes: Vec::new(),
            playout_deadline: Instant::now(),
            frames,
            output_cancellation: CancellationToken::new(),
        }
    }

    pub(super) fn interrupt(&mut self) {
        if let Some(record) = self.cancel_latency_marker() {
            super::latency::log(&record);
        }
        self.tool_ack_active = false;
        self.output_cancellation.cancel();
        self.output_cancellation = CancellationToken::new();
        self.source.clear_buffer();
        self.pending_bytes.clear();
        self.playout_deadline = Instant::now();
    }

    fn cancel_latency_marker(&mut self) -> Option<serde_json::Value> {
        let marker = self.latency_marker.take()?;
        let now = Instant::now();
        Some(first_frame_record(marker, now, None, now))
    }

    pub(super) fn is_playing(&self) -> bool {
        self.playing_at(Instant::now())
    }

    /// `now` is a parameter rather than an `Instant::now()` inside, so the
    /// moment playout ends can be asked about: a clock read in here can be
    /// compared with `<` or `<=` and no test could ever tell, because it
    /// cannot land on the deadline exactly.
    fn playing_at(&self, now: Instant) -> bool {
        now < self.playout_deadline
    }

    /// Nothing playing and nothing of a reply waiting to be: no frame still
    /// ahead of the playout deadline, no chunk too short to make a frame, and
    /// no first-audio marker waiting for one. Audio queued now would be heard
    /// alone.
    pub(super) fn idle_at(&self, now: Instant) -> bool {
        !self.playing_at(now) && self.pending_bytes.is_empty() && self.latency_marker.is_none()
    }

    /// Whether `capture` would queue anything for this chunk.
    ///
    /// Split out, through `prepare_reply`, so the caller can ask before
    /// dropping a turn that is still playing. Dropping first and then finding
    /// the new chunk unusable cuts the
    /// interviewer off mid-sentence with nothing behind it.
    pub(super) fn accepts(&self, bytes: &[u8], mime_type: &str) -> bool {
        !bytes.is_empty()
            && audio_sample_rate(mime_type, self.sample_rate) == Some(self.sample_rate)
    }

    /// Whether this chunk, with what an earlier one left short of a frame,
    /// completes a frame for the worker. A reply's latency marker waits for
    /// that chunk, so the wait for it is counted as the provider's rather than
    /// as output queueing.
    pub(super) fn fills_frame(&self, bytes: &[u8]) -> bool {
        self.pending_bytes.len() + bytes.len()
            >= pcm16_frame_bytes(self.sample_rate, LIVEKIT_OUTPUT_CHANNELS)
    }

    /// The silence that completes a frame from what a finished reply left
    /// short of one, if it left anything. Left alone, that tail would never
    /// play, would hold the output from idle so the next acknowledgement
    /// could not start, and would be spliced onto the front of the next reply.
    pub(super) fn tail_padding(&self) -> Option<Vec<u8>> {
        let frame = pcm16_frame_bytes(self.sample_rate, LIVEKIT_OUTPUT_CHANNELS);
        let short = self.pending_bytes.len() % frame;
        (short > 0).then(|| vec![0; frame - short])
    }

    /// Whether a reply chunk will be queued, cutting any acknowledgement still
    /// queued or playing first, before the reply's queue is measured or its
    /// marker installed: the reply never waits for the filler to finish. The
    /// filler went out into idle output, so everything the cut cancels is the
    /// filler's.
    pub(super) fn prepare_reply(&mut self, bytes: &[u8], mime_type: &str) -> bool {
        if !self.accepts(bytes, mime_type) {
            return false;
        }
        if self.tool_ack_active {
            self.interrupt();
        }
        true
    }

    /// Queues the acknowledgement as filler that the reply's first chunk cuts.
    pub(super) async fn capture_tool_ack(
        &mut self,
        bytes: &[u8],
        mime_type: &str,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        self.tool_ack_active |= self.accepts(bytes, mime_type);
        self.capture(bytes, mime_type).await
    }

    pub(super) async fn capture(
        &mut self,
        bytes: &[u8],
        mime_type: &str,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        if !self.accepts(bytes, mime_type) {
            return Ok(false);
        }
        let mut queued = false;
        for samples in take_pcm16_frames(
            bytes,
            self.sample_rate,
            LIVEKIT_OUTPUT_CHANNELS,
            &mut self.pending_bytes,
        ) {
            queued = true;

            // Ten milliseconds, which is what `take_pcm16_frames` cut this
            // into. Dividing the sample count by the channel count says the
            // same thing, but the channel count is one, so it says it with an
            // operation that reads identically whichever way round it is
            // written and cannot be wrong in a way anything could observe.
            let samples_per_channel = self.sample_rate / 100;
            let frame_duration =
                Duration::from_secs_f64(samples_per_channel as f64 / self.sample_rate as f64);
            self.playout_deadline = self.playout_deadline.max(Instant::now()) + frame_duration;
            let frame = QueuedOutputFrame {
                output_cancellation: self.output_cancellation.clone(),
                samples,
                latency_marker: self.latency_marker.take(),
            };
            if let Err(error) = self.frames.try_send(frame) {
                let message = match &error {
                    mpsc::error::TrySendError::Full(_) => "output audio worker queue full",
                    mpsc::error::TrySendError::Closed(_) => "output audio worker stopped",
                };
                if let Some(marker) = error.into_inner().latency_marker {
                    let record = enqueue_failure_record(marker, Instant::now());
                    super::latency::log(&record);
                }
                return Err(std::io::Error::other(message).into());
            }
        }
        Ok(queued)
    }
}

/// The native source as the output worker uses it, so a test can stand in a
/// sink whose capture blocks or completes as cancellation lands.
trait OutputAudioSink {
    fn capture_frame(
        &self,
        frame: &AudioFrame<'_>,
    ) -> impl Future<Output = Result<(), Box<dyn std::error::Error + Send + Sync>>> + Send;
    fn clear_buffer(&self);
}

impl OutputAudioSink for NativeAudioSource {
    async fn capture_frame(
        &self,
        frame: &AudioFrame<'_>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        NativeAudioSource::capture_frame(self, frame).await?;
        Ok(())
    }

    fn clear_buffer(&self) {
        NativeAudioSource::clear_buffer(self);
    }
}

async fn output_audio_worker(
    source: impl OutputAudioSink,
    sample_rate: u32,
    mut frames: mpsc::Receiver<QueuedOutputFrame>,
) {
    while let Some(frame) = frames.recv().await {
        if frame.output_cancellation.is_cancelled() {
            if let Some(marker) = frame.latency_marker {
                log_first_frame(marker, Instant::now(), None);
            }
            continue;
        }
        let marker = frame.latency_marker;
        let dequeued_at = marker.as_ref().map(|_| Instant::now());
        let cancelled = frame.output_cancellation.cancelled();
        tokio::pin!(cancelled);
        // Ten milliseconds, as cut by `take_pcm16_frames`. See `capture`.
        let samples_per_channel = sample_rate / 100;
        let audio_frame = AudioFrame {
            data: frame.samples.into(),
            sample_rate,
            num_channels: LIVEKIT_OUTPUT_CHANNELS,
            samples_per_channel,
        };
        tokio::select! {
            biased;
            _ = &mut cancelled => {
                if let (Some(marker), Some(dequeued_at)) = (marker, dequeued_at) {
                    log_first_frame(marker, dequeued_at, None);
                }
                source.clear_buffer();
            }
            result = source.capture_frame(&audio_frame) => {
                if let (Some(marker), Some(dequeued_at)) = (marker, dequeued_at) {
                    log_first_frame(marker, dequeued_at, Some(result.is_ok()));
                }
                if let Err(error) = result {
                    eprintln!("failed to publish output audio frame: {error}");
                }

                // Capture can enqueue after the producer's clear and complete
                // as cancellation arrives. Clear before capturing a new token.
                if frame.output_cancellation.is_cancelled() {
                    source.clear_buffer();
                }
            }
        }
    }
}

/// The reply's first frame left the worker: captured, failed (`Some(false)`),
/// or cancelled before capture (`None`), so an accepted reply never vanishes
/// from the record without a terminal outcome.
fn enqueue_failure_record(marker: (serde_json::Value, Instant), now: Instant) -> serde_json::Value {
    let mut record = first_frame_record(marker, now, None, now);
    record["stage"] = "audio_enqueue_failed".into();
    record["enqueued"] = false.into();
    record
}

fn log_first_frame(
    (record, accepted_at): (serde_json::Value, Instant),
    dequeued_at: Instant,
    captured: Option<bool>,
) {
    let record = first_frame_record((record, accepted_at), dequeued_at, captured, Instant::now());
    super::latency::log(&record);
}

fn first_frame_record(
    (mut record, accepted_at): (serde_json::Value, Instant),
    dequeued_at: Instant,
    captured: Option<bool>,
    now: Instant,
) -> serde_json::Value {
    let since_accepted =
        i64::try_from(crate::agent::saturating_ms(now - accepted_at)).unwrap_or(i64::MAX);
    let total = record["elapsed_ms"]
        .as_i64()
        .map(|ms| ms.saturating_add(since_accepted));
    record["stage"] = match captured {
        Some(_) => "native_audio_capture",
        None => "native_audio_cancelled",
    }
    .into();
    record["output_queue_ms"] =
        crate::agent::saturating_ms(dequeued_at.saturating_duration_since(accepted_at)).into();
    record["native_capture_ms"] =
        crate::agent::saturating_ms(now.saturating_duration_since(dequeued_at)).into();
    record["server_total_ms"] = total.into();
    if let Some(observed) = record["observed_ms"].as_i64() {
        record["observed_ms"] = observed.saturating_add(since_accepted).into();
    }
    record["captured"] = captured.unwrap_or(false).into();
    record["native_queue_bound_ms"] = LIVEKIT_OUTPUT_QUEUE_MS.into();
    record
}

/// How the agent's own voice is published.
///
/// Named as the microphone rather than left to default, because a track with
/// no source is not the one a client picks when it looks for who is speaking:
/// the browser subscribes by source, and Meet presentation mode routes on it.
fn agent_voice_track_options() -> TrackPublishOptions {
    TrackPublishOptions {
        source: TrackSource::Microphone,
        ..Default::default()
    }
}

pub(super) async fn publish_output_audio(
    room: &Room,
    sample_rate: u32,
) -> Result<OutputAudio, Box<dyn std::error::Error + Send + Sync>> {
    let source = NativeAudioSource::new(
        AudioSourceOptions::default(),
        sample_rate,
        LIVEKIT_OUTPUT_CHANNELS,
        LIVEKIT_OUTPUT_QUEUE_MS,
    );
    let track = LocalAudioTrack::create_audio_track(
        "interviewer-audio",
        RtcAudioSource::Native(source.clone()),
    );
    room.local_participant()
        .publish_track(LocalTrack::Audio(track), agent_voice_track_options())
        .await?;
    let (frames, queued_frames) = mpsc::channel(LIVEKIT_OUTPUT_FRAME_QUEUE);
    tokio::spawn(output_audio_worker(
        source.clone(),
        sample_rate,
        queued_frames,
    ));
    Ok(OutputAudio::new(source, sample_rate, frames))
}

/// Root-mean-square level, in PCM16 units, above which a frame counts as the
/// candidate speaking: about -40 dBFS, the top of a quiet room and below soft
/// speech. Used to hold a checkpoint back, and a silence nudge while the socket
/// has not transcribed the candidate, so it errs toward calling sound speech: a
/// noisy room waits longer, which the watch tick retries, where a soft speaker
/// taken for silence would be interrupted.
const VOICE_RMS: f64 = 316.0;

pub(super) fn frame_has_voice(frame: &AudioFrame<'_>) -> bool {
    pcm16_has_voice(&frame.data)
}

fn pcm16_has_voice(samples: &[i16]) -> bool {
    if samples.is_empty() {
        return false;
    }
    let energy = samples
        .iter()
        .map(|sample| f64::from(*sample).powi(2))
        .sum::<f64>();
    (energy / samples.len() as f64).sqrt() > VOICE_RMS
}

pub(super) fn append_pcm16_bytes(frame: &AudioFrame<'_>, bytes: &mut Vec<u8>) {
    frame
        .data
        .iter()
        .for_each(|sample| bytes.extend(sample.to_le_bytes()));
}

fn audio_sample_rate(mime_type: &str, default_pcm_rate: u32) -> Option<u32> {
    mime_type
        .split(';')
        .find_map(|part| {
            part.trim()
                .strip_prefix("rate=")
                .and_then(|rate| rate.parse().ok())
        })
        .or_else(|| (mime_type.trim() == "audio/pcm").then_some(default_pcm_rate))
}

pub(super) fn take_pcm16_frames(
    bytes: &[u8],
    sample_rate: u32,
    channels: u32,
    pending: &mut Vec<u8>,
) -> Vec<Vec<i16>> {
    let frame_bytes = pcm16_frame_bytes(sample_rate, channels);
    if frame_bytes == 0 {
        return Vec::new();
    }

    pending.extend_from_slice(bytes);
    let complete_len = pending.len() - pending.len() % frame_bytes;
    let frames = pending[..complete_len]
        .chunks_exact(frame_bytes)
        .map(decode_pcm16)
        .collect();
    pending.drain(..complete_len);
    frames
}

/// The bytes in one ten-millisecond PCM16 frame.
fn pcm16_frame_bytes(sample_rate: u32, channels: u32) -> usize {
    (sample_rate / 100 * channels) as usize * 2
}

fn decode_pcm16(bytes: &[u8]) -> Vec<i16> {
    bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|chunk| i16::from_le_bytes([chunk[0], chunk[1]]))
        .collect()
}

fn should_send_video_frame(
    elapsed: Duration,
    candidate_speaking: bool,
    presence_sent: bool,
) -> bool {
    elapsed >= GEMINI_VIDEO_FRAME_INTERVAL && (candidate_speaking || !presence_sent)
}

/// JPEG sides and RGBA byte count for a frame, or `None` when the dimensions
/// are absurd. Both bounds live here because they answer one question and a
/// frame that clears the size cap can still overflow a 16-bit JPEG side, while
/// a 100000x1 frame sits under the cap and still has no valid SOF.
///
/// Bounded in bytes rather than in pixels, because bytes are what is actually
/// at stake: this is the buffer the frame is unpacked into before it is
/// encoded. A pixel cap set past 8K allowed a quarter of a gigabyte for one
/// frame off a header this process does not control. The budget below fits 4K
/// with room to spare, which is past any camera a candidate is interviewing on.
fn jpeg_frame_geometry(width: u32, height: u32) -> Option<(u16, u16, usize)> {
    const MAX_RGBA_BYTES: usize = 64 * 1024 * 1024;
    let pixels = width.checked_mul(height)?;
    if pixels == 0 {
        return None;
    }
    let bytes = (pixels as usize).checked_mul(4)?;
    (bytes <= MAX_RGBA_BYTES).then_some(())?;
    Some((
        u16::try_from(width).ok()?,
        u16::try_from(height).ok()?,
        bytes,
    ))
}

/// Converts a frame to packed R, G, B, A bytes.
///
/// The format name is a trap. libyuv names its 32-bit formats after the
/// little-endian word rather than the byte order, so `ABGR` is the variant that
/// writes R, G, B, A into memory and `RGBA` writes A, B, G, R. Asking for
/// `RGBA` pins the red channel to the alpha constant and transposes green and
/// blue, which `channel_order_survives_the_libyuv_naming_trap` pins down.
pub(super) fn frame_to_rgba(frame: &BoxVideoFrame) -> Result<(Vec<u8>, u16, u16), String> {
    let width = frame.buffer.as_ref().width();
    let height = frame.buffer.as_ref().height();

    // Remote-controlled dimensions: overflow here would panic in debug and
    // under-allocate the buffer `to_argb` writes into in release.
    let Some((jpeg_width, jpeg_height, rgba_len)) = jpeg_frame_geometry(width, height) else {
        return Err(format!(
            "frame {width}x{height} is outside JPEG limits (each side at most 65535, and at most 64 MiB unpacked: 16777216 pixels in all)"
        ));
    };
    let mut rgba = vec![0; rgba_len];
    frame.buffer.as_ref().to_argb(
        VideoFormatType::ABGR,
        &mut rgba,
        width * 4,
        width as i32,
        height as i32,
    );
    Ok((rgba, jpeg_width, jpeg_height))
}

/// The compression on its own, so the caller can put it somewhere other than
/// the executor. Owned pixels rather than the frame, because the frame does not
/// cross a thread and this does.
pub(super) fn encode_rgba_jpeg(
    rgba: &[u8],
    jpeg_width: u16,
    jpeg_height: u16,
    quality: u8,
) -> Result<Vec<u8>, String> {
    // Chroma is subsampled 2x2 here, because that is what `Encoder::new` picks
    // below quality 90 and nothing below overrides it. Deliberate: the source
    // is I420, whose chroma is already 4:2:0, so encoding it at full resolution
    // would spend about a fifth of the payload on interpolated values. The
    // override, if a future frame source is not I420, is
    // `set_sampling_factor(SamplingFactor::F_1_1)` before the call.
    let mut jpeg = Vec::new();
    Encoder::new(&mut jpeg, quality)
        .encode(rgba, jpeg_width, jpeg_height, ColorType::Rgba)
        .map_err(|error| error.to_string())?;
    Ok(jpeg)
}

/// A frame to JPEG bytes, with the compression moved off the executor.
///
/// The colour conversion stays inline: it is libyuv, which is compiled
/// optimized whatever profile this crate is built at, and the frame it borrows
/// does not cross a thread. The JPEG pass is the one that costs, and it is pure
/// Rust: measured at 6.8ms per 720p frame in release and 138ms in a dev build.
/// It shares an executor with Gemini's audio events, so at a frame a second
/// that stall lands in the middle of a conversation and the candidate hears it
/// as the reply arriving late.
pub(super) async fn encode_video_frame_jpeg_off_thread(
    frame: &BoxVideoFrame,
    quality: u8,
) -> Result<Vec<u8>, String> {
    let (rgba, jpeg_width, jpeg_height) = frame_to_rgba(frame)?;
    tokio::task::spawn_blocking(move || encode_rgba_jpeg(&rgba, jpeg_width, jpeg_height, quality))
        .await
        .map_err(|error| format!("jpeg encode task did not finish: {error}"))?
}

#[cfg(test)]
#[path = "../../tests/unit/livekit/media.rs"]
mod tests;
